use std::borrow::Cow;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;

use anyhow::{Context as _, Error};
use lalrpop_util::ParseError;
use rand::seq::SliceRandom;
use regex::{Captures, Regex, Replacer};
use sea_orm::sea_query::{ConditionExpression, Expr, Func, PgFunc, SimpleExpr};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait,
    ModelTrait, QueryFilter, QuerySelect, QueryTrait, Statement,
};
use time::macros::format_description;
use time::Date;
use tokio::sync::OnceCell;
use twilight_cache_inmemory::InMemoryCache;
use twilight_http::Client as DiscordClient;
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::Message;
use twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder};
use unicode_width::UnicodeWidthStr;

use crate::command_parser::{Access, Args, CommandHandler, Commands, Help};
use crate::config::Config;
use crate::models::{game, game_entry, quote, show};

// regconfig for `english`
static ENGLISH: OnceCell<u32> = OnceCell::const_new();

#[derive(Debug, PartialEq, Eq, Copy, Clone, PartialOrd, Ord)]
pub enum Op {
    /// The `:` operator.
    Fuzzy,
    /// The `<` operator.
    Less,
    /// The `=` operator.
    Equal,
    /// The `>` operator.
    Greater,
    /// The `<=` operator.
    LessEqual,
    /// The `>=` operator.
    GreaterEqual,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Column {
    Context,
    Date,
    Game,
    Id,
    Name,
    Quote,
    Show,
}

impl Column {
    /// Does the fuzzy match on this column use the full-text search?
    fn fuzzy_is_fts(self) -> bool {
        match self {
            Column::Context => true,
            Column::Date => false,
            Column::Game => false,
            Column::Id => false,
            Column::Name => false,
            Column::Quote => true,
            Column::Show => false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Ast<'input> {
    Or { exprs: Vec<Ast<'input>> },
    And { exprs: Vec<Ast<'input>> },
    Column { column: Column, op: Op, term: Cow<'input, str> },
    Bare(Cow<'input, str>),
}

fn as_ilike(s: &str) -> String {
    lazy_static::lazy_static! {
        static ref RE_BOUNDARY: Regex = Regex::new(r"^|\s+|$").unwrap();
        static ref RE_METACHARS: Regex = Regex::new(r"([\\%_])").unwrap();
    }
    let s = RE_METACHARS.replace_all(s, "\\$1");
    RE_BOUNDARY.replace_all(&s, "%").into_owned()
}

fn single_predicate<C: ColumnTrait, T: Into<sea_orm::Value>>(
    column: C,
    op: Op,
    value: T,
    fuzzy: impl FnOnce(C, T) -> SimpleExpr,
) -> SimpleExpr {
    match op {
        Op::Fuzzy => fuzzy(column, value),
        Op::Equal => column.eq(value),
        Op::Less => column.lt(value),
        Op::LessEqual => column.lte(value),
        Op::Greater => column.gt(value),
        Op::GreaterEqual => column.gte(value),
    }
}

impl<'a> Ast<'a> {
    fn and(self, right: Ast<'a>) -> Ast<'a> {
        match (self, right) {
            (mut left @ Ast::And { .. }, Ast::And { exprs }) => {
                for expr in exprs {
                    left = left.and(expr);
                }
                left
            }
            (right, Ast::And { mut exprs }) | (Ast::And { mut exprs }, right) => {
                match right {
                    Ast::Column { column, op: Op::Fuzzy, term } if column.fuzzy_is_fts() => {
                        let mut merged = false;
                        for expr in &mut exprs {
                            *expr = match *expr {
                                Ast::Column {
                                    column: l_column,
                                    op: Op::Fuzzy,
                                    term: Cow::Borrowed(ref left),
                                } if l_column == column => Ast::Column {
                                    column,
                                    op: Op::Fuzzy,
                                    term: Cow::Owned(format!("{} {}", left, term)),
                                },
                                Ast::Column {
                                    column: l_column,
                                    op: Op::Fuzzy,
                                    term: Cow::Owned(ref mut left),
                                } if l_column == column => {
                                    left.push(' ');
                                    left.push_str(&term);
                                    merged = true;
                                    break;
                                }
                                _ => continue,
                            };
                            merged = true;
                            break;
                        }
                        if !merged {
                            exprs.push(Ast::Column { column, op: Op::Fuzzy, term });
                        }
                    }
                    Ast::Bare(term) => {
                        let mut merged = false;
                        for expr in &mut exprs {
                            *expr = match expr {
                                Ast::Bare(Cow::Borrowed(orig)) => {
                                    Ast::Bare(Cow::Owned(format!("{} {}", orig, term)))
                                }
                                Ast::Bare(Cow::Owned(orig)) => {
                                    orig.push(' ');
                                    orig.push_str(&term);
                                    merged = true;
                                    break;
                                }
                                _ => continue,
                            };
                            merged = true;
                            break;
                        }
                        if !merged {
                            exprs.push(Ast::Bare(term));
                        }
                    }
                    right => exprs.push(right),
                }
                Ast::And { exprs }
            }
            (
                Ast::Column { column: l_column, op: Op::Fuzzy, term: ref l_term },
                Ast::Column { column: r_column, op: Op::Fuzzy, term: ref r_term },
            ) if l_column == r_column && l_column.fuzzy_is_fts() => Ast::Column {
                column: l_column,
                op: Op::Fuzzy,
                term: Cow::Owned(format!("{} {}", l_term, r_term)),
            },
            (Ast::Bare(left), Ast::Bare(right)) => {
                Ast::Bare(Cow::Owned(format!("{} {}", left, right)))
            }
            (left, right) => Ast::And { exprs: vec![left, right] },
        }
    }

    fn or(self, right: Ast<'a>) -> Ast<'a> {
        match (self, right) {
            (mut left @ Ast::Or { .. }, Ast::Or { exprs }) => {
                for expr in exprs {
                    left = left.or(expr);
                }
                left
            }
            (right, Ast::Or { mut exprs }) | (Ast::Or { mut exprs }, right) => {
                exprs.push(right);
                Ast::Or { exprs }
            }
            (left, right) => Ast::Or { exprs: vec![left, right] },
        }
    }

    fn to_condition(&self) -> Result<ConditionExpression, Error> {
        match self {
            Ast::Or { exprs } => {
                let mut cond = Condition::any();
                for node in exprs {
                    cond = cond.add(node.to_condition()?);
                }
                Ok(cond.into())
            }
            Ast::And { exprs } => {
                let mut cond = Condition::all();
                for node in exprs {
                    cond = cond.add(node.to_condition()?);
                }
                Ok(cond.into())
            }
            Ast::Column { column, op, term } => match column {
                Column::Id => {
                    let term = term
                        .parse::<i32>()
                        .with_context(|| format!("failed to parse {term:?} as an integer"))?;

                    Ok(single_predicate(quote::Column::Id, *op, term, |c, v| c.eq(v)).into())
                }
                Column::Quote => {
                    Ok(single_predicate(quote::Column::Quote, *op, &term[..], |c, v| {
                        Expr::expr(PgFunc::to_tsvector(Expr::col(c), ENGLISH.get().copied()))
                            .matches(PgFunc::plainto_tsquery(Expr::val(v), ENGLISH.get().copied()))
                    })
                    .into())
                }
                Column::Name => {
                    Ok(single_predicate(quote::Column::AttribName, *op, &term[..], |c, v| {
                        // TODO: `sea_query` has `LIKE` but not `ILIKE`
                        Expr::expr(Func::lower(Expr::col(c))).like(as_ilike(&v).to_lowercase())
                    })
                    .into())
                }
                Column::Date => {
                    let term = Date::parse(term, format_description!("[year]-[month]-[day]"))
                        .with_context(|| format!("failed to parse {term:?} as a date"))?;
                    Ok(single_predicate(quote::Column::AttribDate, *op, term, |c, v| c.eq(v))
                        .into())
                }
                Column::Context => {
                    Ok(single_predicate(quote::Column::Context, *op, &term[..], |c, v| {
                        c.is_not_null().and(
                            Expr::expr(PgFunc::to_tsvector(
                                Func::coalesce([Expr::col(c), Expr::val("")]),
                                ENGLISH.get().copied(),
                            ))
                            .matches(PgFunc::plainto_tsquery(Expr::val(v), ENGLISH.get().copied())),
                        )
                    })
                    .into())
                }
                Column::Game => {
                    Ok(Expr::col(quote::Column::GameId)
                        .in_subquery(
                            QuerySelect::query(
                                &mut game::Entity::find()
                                    .filter(single_predicate(
                                        game::Column::Name,
                                        *op,
                                        &term[..],
                                        |c, v| {
                                            // TODO: `sea_query` has `LIKE` but not `ILIKE`
                                            Expr::expr(Func::lower(Expr::col(c)))
                                                .like(as_ilike(&v).to_lowercase())
                                        },
                                    ))
                                    .select_only()
                                    .column(game::Column::Id),
                            )
                            .take(),
                        )
                        .into())
                }
                Column::Show => {
                    Ok(Expr::col(quote::Column::ShowId)
                        .in_subquery(
                            QuerySelect::query(
                                &mut show::Entity::find()
                                    .filter(single_predicate(
                                        show::Column::Name,
                                        *op,
                                        &term[..],
                                        |c, v| {
                                            // TODO: `sea_query` has `LIKE` but not `ILIKE`
                                            Expr::expr(Func::lower(Expr::col(c)))
                                                .like(as_ilike(&v).to_lowercase())
                                        },
                                    ))
                                    .select_only()
                                    .column(show::Column::Id),
                            )
                            .take(),
                        )
                        .into())
                }
            },
            Ast::Bare(term) => Ok(Expr::expr(PgFunc::to_tsvector(
                Expr::col(quote::Column::Quote).concatenate(Expr::val(" ")).concatenate(
                    Func::coalesce([Expr::col(quote::Column::Context), Expr::val("")]),
                ),
                ENGLISH.get().copied(),
            ))
            .matches(PgFunc::plainto_tsquery(Expr::val(&term[..]), ENGLISH.get().copied()))
            .into()),
        }
    }
}

fn unescape(s: &str) -> Cow<str> {
    lazy_static::lazy_static! {
        static ref RE_ESCAPE: Regex = Regex::new(r"\\(.)").unwrap();
    }

    struct Expander;

    impl Replacer for Expander {
        fn replace_append(&mut self, captures: &Captures, dst: &mut String) {
            match captures.get(1).unwrap().as_str() {
                "n" => dst.push_str("\n"),
                "r" => dst.push_str("\r"),
                "t" => dst.push_str("\t"),
                c => dst.push_str(c),
            }
        }
    }

    assert!(s.starts_with('\"') && s.ends_with('\"'));
    RE_ESCAPE.replace_all(&s[1..s.len() - 1], Expander)
}

fn parse_emoji(emoji: &str) -> &str {
    lazy_static::lazy_static! {
        static ref RE_EMOJI: Regex = Regex::new(r"^<:(\w+):\d+>$").unwrap();
    }

    RE_EMOJI.captures(emoji).unwrap().get(1).unwrap().as_str()
}

fn parse_emoji_name(emoji: &str) -> &str {
    lazy_static::lazy_static! {
        static ref RE_EMOJI_NAME: Regex = Regex::new(r"^:(\w+):$").unwrap();
    }

    RE_EMOJI_NAME.captures(emoji).unwrap().get(1).unwrap().as_str()
}

lalrpop_util::lalrpop_mod!(#[allow(clippy::all)] pub parser, "/commands/quote.rs");

async fn report_parse_error(
    discord: &DiscordClient,
    message: &Message,
    query: &str,
    err: ParseError<usize, parser::Token<'_>, Infallible>,
) -> Result<(), Error> {
    let (start, end) = match &err {
        ParseError::InvalidToken { location } => (*location, *location),
        ParseError::UnrecognizedEOF { location, .. } => (*location, *location),
        ParseError::UnrecognizedToken { token: (start, _, end), .. } => (*start, *end),
        ParseError::ExtraToken { token: (start, _, end) } => (*start, *end),
        ParseError::User { error } => match *error {},
    };

    let query = query.replace('\n', "\u{2424}");

    let lead_width = query[..start].width();
    let caret_width = std::cmp::max(query[start..end].width(), 1);

    let mut caret_line = String::with_capacity(lead_width + caret_width);
    for _ in 0..lead_width {
        caret_line.push(' ');
    }
    for _ in 0..caret_width {
        caret_line.push('^');
    }

    discord
        .create_message(message.channel_id)
        .reply(message.id)
        .flags(MessageFlags::SUPPRESS_EMBEDS)
        .content(&format!(
            "Failed to parse the query: {}\n```{}\n{caret_line}```",
            crate::markdown::escape(&err.to_string()),
            crate::markdown::escape_code_block(&query),
        ))
        .context("error report invalid")?
        .await
        .context("failed to report the parse error")?;

    Ok(())
}

async fn load_regconfig(conn: &DatabaseConnection) -> Result<(), Error> {
    ENGLISH
        .get_or_try_init::<Error, _, _>(|| async {
            let row = conn
                .query_one(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "SELECT 'english'::REGCONFIG::OID AS english",
                    [],
                ))
                .await
                .context("failed to query the `english` regconfig")?
                .context("`english` regconfig missing")?;
            Ok(row.try_get("", "english").context("failed to get the column")?)
        })
        .await?;
    Ok(())
}

pub struct Find {
    db: DatabaseConnection,
}

impl Find {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

impl CommandHandler for Find {
    fn pattern(&self) -> &str {
        "(?:find)?quote(?: (.+))?"
    }

    fn help(&self) -> Option<crate::command_parser::Help> {
        Some(Help {
            name: "quote".into(),
            usage: "quote [ID | QUERY]".into(),
            summary: "Search for a quote in the quote database".into(),
            description: concat!(
                "Search for a quote in the quote database.\n",
                "\n",
                "You can search for a quote by its ID or by using the query language.\n\n",
                "The query language is designed such that you can still type words in and get ",
                "vaguely relevant quotes back.\n",
                "\n",
                "A query is broken up into terms. A term is either an unquoted word ",
                "(eg. `butts`), a quoted phrase (eg. `\"my butt\"`), or a column name (`context`, ",
                "`date`, `from`/`name`, `game`, `id`, `quote`/`text`, `show`) followed by an ",
                "operator (the fuzzy search operator `:` or a relational operator `<`, `=`, `>`, ",
                "`>=`, `<=`) followed by an unquoted word or a quoted phrase (eg. `quote:butts`).\n",
                "\n",
                "Multiple terms can be combined together to form a more complex query. By default ",
                "when you write two terms one after the other both need to match the quote ",
                "(boolean AND). If the two terms are separated by a `|` then either of them needs ",
                "to match the quote (boolean OR). AND has higher precedence than OR but you can ",
                "use parentheses to override that.\n",
                "\n",
                "When a query matches multiple quotes a random one is picked. An empty query ",
                "matches all quotes.\n",
                "\n",
                "Please keep in mind that many of the quotes are taken out of context, be it for ",
                "comedic effect or out of necessity. Take all of them with a grain of salt and ",
                "bear in mind they don't necessarily reflect their originators' views and ",
                "opinions. That being said, if you find any quote to be particularly awful, ",
                "please notify the moderator of your choice to have its removal evaluated.",
            ).into(),
            examples: Cow::Borrowed(&[
                Cow::Borrowed("quote "),
                Cow::Borrowed("quote 3849"),
                Cow::Borrowed("quote findquote butts"),
                Cow::Borrowed("quote context:pants"),
                Cow::Borrowed("quote from:alex butts"),
                Cow::Borrowed("quote id < 1000"),
                Cow::Borrowed("quote date >= 2019-01-01"),
                Cow::Borrowed(concat!(
                    "quote ",
                    "(show:\"IDDQDerp\" | show:\"Let's NOPE\" | show:\"Watch and Play\") ",
                    "from:Alex \"long pig\"",
                )),
            ]),
        })
    }

    fn handle<'a>(
        &'a self,
        _: &'a InMemoryCache,
        _: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            load_regconfig(&self.db).await.context("failed to load `english` regconfig")?;

            let query = args.get(0).unwrap_or("");
            let quotes = if query.is_empty() {
                quote::Entity::find()
                    .filter(Expr::col(quote::Column::Deleted).not())
                    .all(&self.db)
                    .await?
            } else if let Ok(id) = query.parse::<i32>() {
                quote::Entity::find_by_id(id)
                    .filter(Expr::col(quote::Column::Deleted).not())
                    .all(&self.db)
                    .await?
            } else {
                let parser = parser::QueryParser::new();
                let query = match parser.parse(query) {
                    Ok(query) => query,
                    Err(err) => return report_parse_error(discord, message, query, err).await,
                };
                quote::Entity::find()
                    .filter(
                        Condition::all()
                            .add(query.to_condition()?)
                            .add(Expr::col(quote::Column::Deleted).not()),
                    )
                    .all(&self.db)
                    .await?
            };

            let quote = quotes.choose(&mut rand::thread_rng());

            let content;
            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .flags(MessageFlags::SUPPRESS_EMBEDS)
                .content(match quote {
                    Some(quote) => {
                        content = format!("Quote {}", crate::markdown::escape(&quote.to_string()));
                        &content
                    }
                    None => "Could not find any matching quotes.",
                })
                .context("command response invalid")?
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}

pub struct QueryDebugger;

impl QueryDebugger {
    pub fn new() -> Self {
        Self
    }
}

impl CommandHandler for QueryDebugger {
    fn pattern(&self) -> &str {
        "quote query-debugger(?: (.+))"
    }

    fn help(&self) -> Option<Help> {
        None
    }

    fn access(&self) -> Access {
        Access::ModOnly
    }

    fn handle<'a>(
        &'a self,
        _: &'a InMemoryCache,
        _: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let query = args.get(0).unwrap_or("");
            let content;

            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .flags(MessageFlags::SUPPRESS_EMBEDS)
                .content(if query.is_empty() {
                    "Query: pick a random quote"
                } else if let Ok(id) = query.parse::<i32>() {
                    content = format!("Query: fetch quote #{}", id);
                    &content
                } else {
                    let parser = parser::QueryParser::new();
                    let query = match parser.parse(query) {
                        Ok(query) => query,
                        Err(err) => return report_parse_error(discord, message, query, err).await,
                    };

                    let sql = quote::Entity::find()
                        .filter(
                            Condition::all()
                                .add(query.to_condition()?)
                                .add(Expr::col(quote::Column::Deleted).not()),
                        )
                        .build(DatabaseBackend::Postgres)
                        .to_string();

                    content = format!(
                        "AST:\n```{}```\nSQL:\n`{}`",
                        crate::markdown::escape_code_block(&format!("{query:#?}")),
                        crate::markdown::escape(&sql),
                    );
                    &content
                })
                .context("command response invalid")?
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}

pub struct Details {
    db: DatabaseConnection,
}

impl Details {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }
}

impl CommandHandler for Details {
    fn pattern(&self) -> &str {
        r"quote details (\d+)"
    }

    fn help(&self) -> Option<Help> {
        Some(Help {
            name: "quote details".into(),
            usage: "quote details <ID>".into(),
            summary: "Post detailed information about a quote".into(),
            description: "Post detailed information about a quote.".into(),
            examples: Cow::Borrowed(&[Cow::Borrowed("quote details 110")]),
        })
    }

    fn handle<'a>(
        &'a self,
        _: &'a InMemoryCache,
        _: &'a Config,
        discord: &'a DiscordClient,
        _: Commands<'a>,
        message: &'a Message,
        args: &'a Args,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let quote_id = match args.get(0).context("quote ID missing")?.parse::<i32>() {
                Ok(id) => id,
                Err(error) => {
                    discord
                        .create_message(message.channel_id)
                        .reply(message.id)
                        .flags(MessageFlags::SUPPRESS_EMBEDS)
                        .content(&format!("Failed to parse the quote ID: {error}"))
                        .context("error report invalid")?
                        .await
                        .context("failed to report the parse error")?;
                    return Ok(());
                }
            };

            let quote = quote::Entity::find_by_id(quote_id)
                .filter(Expr::col(quote::Column::Deleted).not())
                .one(&self.db)
                .await
                .context("failed to load the quote")?;
            let quote = if let Some(quote) = quote {
                quote
            } else {
                discord
                    .create_message(message.channel_id)
                    .reply(message.id)
                    .flags(MessageFlags::SUPPRESS_EMBEDS)
                    .content(&format!("Could not find quote #{}", quote_id))
                    .context("error report invalid")?
                    .await
                    .context("failed to report the parse error")?;
                return Ok(());
            };

            let game = quote
                .find_related(game::Entity)
                .one(&self.db)
                .await
                .context("failed to load the game")?;
            let show = quote
                .find_related(show::Entity)
                .one(&self.db)
                .await
                .context("failed to load the show")?;
            let game_entry = quote
                .find_related(game_entry::Entity)
                .one(&self.db)
                .await
                .context("failed to load the game entry")?;

            let mut embed = EmbedBuilder::new()
                .field(EmbedFieldBuilder::new("ID", quote.id.to_string()))
                .field(EmbedFieldBuilder::new("Quote", crate::markdown::escape(&quote.quote)));
            if let Some(ref name) = quote.attrib_name {
                embed = embed.field(EmbedFieldBuilder::new("Name", crate::markdown::escape(&name)));
            }
            if let Some(date) = quote.attrib_date {
                embed = embed.field(EmbedFieldBuilder::new("Date", date.to_string()));
            }
            if let Some(ref context) = quote.context {
                embed = embed
                    .field(EmbedFieldBuilder::new("Context", crate::markdown::escape(&context)));
            }
            if let Some(game) = game {
                embed = embed.field(EmbedFieldBuilder::new("Game ID", game.id.to_string())).field(
                    EmbedFieldBuilder::new("Game name", crate::markdown::escape(&game.name)),
                );
            }
            if let Some(game_entry) = game_entry {
                if let Some(display_name) = game_entry.display_name {
                    embed = embed.field(EmbedFieldBuilder::new(
                        "Game display name",
                        crate::markdown::escape(&display_name),
                    ));
                }
            }
            if let Some(show) = show {
                embed = embed.field(EmbedFieldBuilder::new("Show ID", show.id.to_string())).field(
                    EmbedFieldBuilder::new("Show name", crate::markdown::escape(&show.name)),
                );
            }
            discord
                .create_message(message.channel_id)
                .reply(message.id)
                .content(&format!("Quote {}", crate::markdown::escape(&quote.to_string())))
                .context("command response invalid")?
                .embeds(&[embed.build()])
                .context("quote details embed invalid")?
                .await
                .context("failed to reply to command")?;

            Ok(())
        })
    }
}

#[cfg(test)]
mod test {
    use std::borrow::Cow;

    use super::parser::QueryParser;
    use super::{as_ilike, unescape, Ast, Column, Op};

    #[test]
    fn parsing() {
        let parser = QueryParser::new();
        assert_eq!(parser.parse("butts").unwrap(), Ast::Bare(Cow::Borrowed("butts")));
        assert_eq!(
            parser.parse("bare words get concatenated").unwrap(),
            Ast::Bare(Cow::Borrowed("bare words get concatenated"))
        );
        assert_eq!(
            parser.parse("quote:also quote:FTS quote:fields").unwrap(),
            Ast::Column {
                column: Column::Quote,
                op: Op::Fuzzy,
                term: Cow::Borrowed("also FTS fields"),
            }
        );
    }

    #[test]
    fn unquote() {
        assert_eq!(unescape("\"test\""), "test");
        assert_eq!(unescape("\"quote: \\\" \\n\""), "quote: \" \n");
    }

    #[test]
    fn ilike() {
        assert_eq!(as_ilike("dark souls"), "%dark%souls%");
        assert_eq!(as_ilike("%"), "%\\%%");
    }
}
