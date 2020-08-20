use crate::extract::Extract;
use crate::models::{Game, GameEntry, Quote, Show};
use crate::pg_fts::{english, plainto_tsquery, to_tsvector};
use crate::schema::{game_per_show_data as game_entries, games, quotes, shows};
use crate::typemap_keys::PgPool;
use chrono::NaiveDate;
use diesel::expression::{AsExpression, NonAggregate};
use diesel::pg::Pg;
use diesel::prelude::*;
use diesel::query_builder::QueryFragment;
use diesel::sql_types::{Bool, Nullable, Text};
use diesel_full_text_search::TsVectorExtensions;
use lalrpop_util::ParseError;
use rand::seq::SliceRandom;
use regex::{Captures, Regex, Replacer};
use serenity::framework::standard::macros::{command, group};
use serenity::framework::standard::{Args, CommandResult};
use serenity::model::prelude::*;
use serenity::prelude::*;
use serenity::utils::MessageBuilder;
use std::borrow::Cow;
use std::convert::Infallible;
use std::fmt::Display;
use unicode_width::UnicodeWidthStr;

// We want to register these commands and also have help texts for all* of them:
//  * `!quote` and `!findquote` => `quote`
//  * `!quote details` => `details`
//  * `!quote query_debugger` => `query_debugger` (* we don't actually want the help text for this)
//
// A single group with `prefixes: ["quote"]` gets us everything except `!findquote` and also
// creates an unnecessary alias for `!quote`.
//
// A single top-level command `!quote` gets us only help text for the top level command and some
// unnecesary aliases `!findquote details` etc.
//
// Subgroups gets us everything we want but parts of this seem very hacky and probably need issues
// filed against Serenity. The user facing downside of this approach is a visible subgroup in
// `!help`.
#[group("Detailed information")]
#[prefix = "quote"]
// Enable matching of the bare `!quote`.
#[default_command(quote)]
#[commands(details, query_debugger)]
struct DetailedInformation;

#[group("Quote")]
#[description = "Commands for querying the quote database.\n\nPlease keep in mind that many of the following quotes are taken out of context, be it for comedic effect or out of necessity. Take all of them with a grain of salt and bear in mind they don't necessarily reflect their originators' views and opinions. That being said, if you find any quote to be particularly awful, please notify the moderator of your choice to have its removal evaluated."]
// The subgroup seems to override matching `!quote` so in effect this only registers the `!findquote` and the help text for `!quote`.
#[commands(quote)]
#[sub_groups(DetailedInformation)]
// `Quote` conflicts with the `Quote` model.
struct QuoteGroup;

pub use self::QUOTEGROUP_GROUP as QUOTE_GROUP;

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

#[derive(Debug, PartialEq, Eq, Copy, Clone, PartialOrd, Ord)]
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
pub enum Expr<'input> {
    Or { exprs: Vec<Expr<'input>> },
    And { exprs: Vec<Expr<'input>> },
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

fn single_predicate<'a, C, T, F, P, Table>(
    column: C,
    op: Op,
    value: T,
    fuzzy: F,
) -> Box<dyn BoxableExpression<Table, Pg, SqlType = Bool> + 'a>
where
    C: AppearsOnTable<Table>
        + SelectableExpression<Table>
        + ExpressionMethods
        + QueryFragment<Pg>
        + NonAggregate
        + 'a,
    T: AsExpression<C::SqlType> + 'a,
    T::Expression:
        AppearsOnTable<Table> + SelectableExpression<Table> + QueryFragment<Pg> + NonAggregate,
    F: FnOnce(C, T) -> P,
    P: BoxableExpression<Table, Pg, SqlType = Bool> + 'a,
{
    match op {
        Op::Fuzzy => Box::new(fuzzy(column, value)),
        Op::Equal => Box::new(column.eq(value)),
        Op::Less => Box::new(column.lt(value)),
        Op::LessEqual => Box::new(column.le(value)),
        Op::Greater => Box::new(column.gt(value)),
        Op::GreaterEqual => Box::new(column.ge(value)),
    }
}

sql_function!(fn coalesce(x: Nullable<Text>, y: Text) -> Text);

impl<'a> Expr<'a> {
    fn and(self, right: Expr<'a>) -> Expr<'a> {
        match (self, right) {
            (mut left @ Expr::And { .. }, Expr::And { exprs }) => {
                for expr in exprs {
                    left = left.and(expr);
                }
                left
            }
            (right, Expr::And { mut exprs }) | (Expr::And { mut exprs }, right) => {
                match right {
                    Expr::Column { column, op: Op::Fuzzy, term } if column.fuzzy_is_fts() => {
                        let mut merged = false;
                        for expr in &mut exprs {
                            *expr = match *expr {
                                Expr::Column {
                                    column: l_column,
                                    op: Op::Fuzzy,
                                    term: Cow::Borrowed(ref left),
                                } if l_column == column => Expr::Column {
                                    column,
                                    op: Op::Fuzzy,
                                    term: Cow::Owned(format!("{} {}", left, term)),
                                },
                                Expr::Column {
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
                            exprs.push(Expr::Column { column, op: Op::Fuzzy, term });
                        }
                    }
                    Expr::Bare(term) => {
                        let mut merged = false;
                        for expr in &mut exprs {
                            *expr = match expr {
                                Expr::Bare(Cow::Borrowed(orig)) => {
                                    Expr::Bare(Cow::Owned(format!("{} {}", orig, term)))
                                }
                                Expr::Bare(Cow::Owned(orig)) => {
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
                            exprs.push(Expr::Bare(term));
                        }
                    }
                    right => exprs.push(right),
                }
                Expr::And { exprs }
            }
            (
                Expr::Column { column: l_column, op: Op::Fuzzy, term: ref l_term },
                Expr::Column { column: r_column, op: Op::Fuzzy, term: ref r_term },
            ) if l_column == r_column && l_column.fuzzy_is_fts() => Expr::Column {
                column: l_column,
                op: Op::Fuzzy,
                term: Cow::Owned(format!("{} {}", l_term, r_term)),
            },
            (Expr::Bare(left), Expr::Bare(right)) => {
                Expr::Bare(Cow::Owned(format!("{} {}", left, right)))
            }
            (left, right) => Expr::And { exprs: vec![left, right] },
        }
    }

    fn or(self, right: Expr<'a>) -> Expr<'a> {
        match (self, right) {
            (mut left @ Expr::Or { .. }, Expr::Or { exprs }) => {
                for expr in exprs {
                    left = left.or(expr);
                }
                left
            }
            (right, Expr::Or { mut exprs }) | (Expr::Or { mut exprs }, right) => {
                exprs.push(right);
                Expr::Or { exprs }
            }
            (left, right) => Expr::Or { exprs: vec![left, right] },
        }
    }

    fn to_predicate(
        &self,
    ) -> Result<Box<dyn BoxableExpression<quotes::table, Pg, SqlType = Bool> + '_>, String> {
        match self {
            Expr::Or { exprs } => {
                let mut iter = exprs.iter();
                let mut ast =
                    iter.next().ok_or_else(|| "empty `Or` node".to_string())?.to_predicate()?;
                for node in iter {
                    ast = Box::new(ast.or(node.to_predicate()?));
                }
                Ok(ast)
            }
            Expr::And { exprs } => {
                let mut iter = exprs.iter();
                let mut ast =
                    iter.next().ok_or_else(|| "empty `And` node".to_string())?.to_predicate()?;
                for node in iter {
                    ast = Box::new(ast.and(node.to_predicate()?));
                }
                Ok(ast)
            }
            Expr::Column { column, op, term } => match column {
                Column::Id => {
                    let term = term.parse::<i32>().map_err(|err| {
                        format!("failed to parse {:?} as an integer: {}", term, err)
                    })?;

                    Ok(single_predicate(quotes::id, *op, term, |c, v| c.eq(v)))
                }
                Column::Quote => Ok(single_predicate(quotes::quote, *op, term, |c, v| {
                    to_tsvector(english(), c).matches(plainto_tsquery(english(), v))
                })),
                Column::Name => Ok(single_predicate(quotes::attrib_name, *op, term, |c, v| {
                    c.ilike(as_ilike(&v))
                })),
                Column::Date => {
                    let term = NaiveDate::parse_from_str(term, "%Y-%m-%d")
                        .map_err(|err| format!("failed to parse {:?} as a date: {}", term, err))?;
                    Ok(single_predicate(quotes::attrib_date, *op, term, |c, v| c.eq(v)))
                }
                Column::Context => Ok(single_predicate(quotes::context, *op, term, |c, v| {
                    c.is_not_null().and(
                        to_tsvector(english(), coalesce(c, ""))
                            .matches(plainto_tsquery(english(), v)),
                    )
                })),
                Column::Game => {
                    let subquery = games::table.select(games::id.nullable()).filter(
                        single_predicate(games::name, *op, term, |c, v| c.ilike(as_ilike(&v))),
                    );
                    Ok(Box::new(quotes::game_id.eq_any(subquery)))
                }
                Column::Show => {
                    let subquery = shows::table.select(shows::id.nullable()).filter(
                        single_predicate(shows::name, *op, term, |c, v| c.ilike(as_ilike(&v))),
                    );
                    Ok(Box::new(quotes::show_id.eq_any(subquery)))
                }
            },
            Expr::Bare(term) => Ok(Box::new(
                to_tsvector(
                    english(),
                    quotes::quote.concat(" ").concat(coalesce(quotes::context, "")),
                )
                .matches(plainto_tsquery(english(), term)),
            )),
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

lalrpop_util::lalrpop_mod!(#[allow(clippy::all)] pub parser, "/commands/quote.rs");

fn safe<T: Display>(val: T) -> String {
    MessageBuilder::new().push_safe(val).build()
}

async fn report_parse_error<'a>(
    msg: &'a Message,
    ctx: &Context,
    query: &str,
    err: ParseError<usize, parser::Token<'a>, Infallible>,
) -> CommandResult {
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

    let message = MessageBuilder::new()
        .push("Failed to parse the query: ")
        .push_safe(err)
        .push_codeblock_safe(format_args!("{}\n{}", query, caret_line), None)
        .build();

    msg.reply(ctx, message).await?;
    Ok(())
}

#[command]
#[aliases(findquote)]
#[usage = "[ID | QUERY]"]
#[example = ""]
#[example = "3849"]
#[example = "findquote butts"]
#[example = "context:pants"]
#[example = "from:alex butts"]
#[example = "id < 1000"]
#[example = "date >= 2019-01-01"]
#[example = "(show:\"IDDQDerp\" | show:\"Let's NOPE\" | show:\"Watch and Play\") from:Alex \"long pig\""]
/// Search for a quote in the quote database.
///
/// You can search for a quote by its ID or by using the query language.
///
/// The query language is designed such that you can still type words in and get vaguely relevant quotes back.
///
/// A query is broken up into terms. A term is either an unquoted word (eg. `butts`), a quoted phrase (eg. `\"my butt\"`), or a column name (`context`, `date`, `from`/`name`, `game`, `id`, `quote`/`text`, `show`) followed by an operator (the fuzzy search operator `:` or a relational operator `<`, `=`, `>`, `>=`, `<=`) followed by an unquoted word or a quoted phrase (eg. `quote:butts`).
///
/// Multiple terms can be combined together to form a more complex query. By default when you write two terms one after the other both need to match the quote (boolean AND). If the two terms are separated by a `|` then either of them needs to match the quote (boolean OR). AND has higher precedence than OR but you can use parentheses to override that.
///
///When a query matches multiple quotes a random one is picked. An empty query matches all quotes.
async fn quote(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    let data = ctx.data.read().await;
    let conn = data.extract::<PgPool>()?.get()?;

    let query = args.rest().trim();
    let quotes = if query.is_empty() {
        quotes::table.filter(diesel::dsl::not(quotes::deleted)).load::<Quote>(&conn)?
    } else if let Ok(id) = query.parse::<i32>() {
        quotes::table.find(id).filter(diesel::dsl::not(quotes::deleted)).load(&conn)?
    } else {
        let parser = parser::QueryParser::new();
        let query = match parser.parse(query) {
            Ok(query) => query,
            Err(err) => return report_parse_error(msg, &ctx, query, err).await,
        };
        let query =
            quotes::table.filter(query.to_predicate()?).filter(diesel::dsl::not(quotes::deleted));
        query.load(&conn)?
    };

    let quote = quotes.choose(&mut rand::thread_rng());
    match quote {
        Some(quote) => {
            let mut builder = MessageBuilder::new();
            builder.push("Quote ");
            builder.push_safe(quote);
            msg.reply(&ctx, builder.build()).await?;
        }
        None => {
            msg.reply(&ctx, "Could not find any matching quotes.").await?;
        }
    }

    Ok(())
}

#[command]
#[required_permissions("ADMINISTRATOR")]
#[help_available(false)]
async fn query_debugger(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    let query = args.rest().trim();

    if query.is_empty() {
        msg.reply(&ctx, "Query: pick a random quote").await?;
    } else if let Ok(id) = query.parse::<i32>() {
        msg.reply(&ctx, format!("Query: fetch quote #{}", id)).await?;
    } else {
        let parser = parser::QueryParser::new();
        let query = match parser.parse(query) {
            Ok(query) => query,
            Err(err) => return report_parse_error(msg, &ctx, query, err).await,
        };

        let message = {
            let predicate = query.to_predicate()?;
            MessageBuilder::new()
                .push("AST: ")
                .push_codeblock_safe(format!("{:#?}", query), None)
                .push("SQL: ")
                .push_mono_safe(diesel::debug_query(&predicate))
                .build()
        };
        msg.reply(&ctx, message).await?;
    }

    Ok(())
}

#[command]
#[description = "Post detailed information about a quote."]
#[usage = "ID"]
#[example = "110"]
#[num_args(1)]
async fn details(ctx: &Context, msg: &Message, args: Args) -> CommandResult {
    let data = ctx.data.read().await;
    let quote_id = match args.parse::<i32>() {
        Ok(id) => id,
        Err(err) => {
            msg.reply(&ctx, format!("Failed to parse the quote ID: {}", err)).await?;
            return Ok(());
        }
    };
    let quote = {
        let conn = data.extract::<PgPool>()?.get()?;

        quotes::table
            .find(quote_id)
            .filter(diesel::dsl::not(quotes::deleted))
            .left_outer_join(games::table)
            .left_outer_join(shows::table)
            .left_outer_join(
                game_entries::table.on(game_entries::game_id
                    .nullable()
                    .eq(quotes::game_id)
                    .and(game_entries::show_id.nullable().eq(quotes::show_id))),
            )
            .first::<(Quote, Option<Game>, Option<Show>, Option<GameEntry>)>(&conn)
            .optional()?
    };
    if let Some((quote, game, show, game_entry)) = quote {
        msg.channel_id
            .send_message(&ctx, |m| {
                let message = MessageBuilder::new()
                    .mention(&msg.author)
                    .push(": Quote ")
                    .push_safe(&quote)
                    .build();
                m.content(message).embed(|embed| {
                    embed.field("ID", safe(quote.id), false).field(
                        "Quote",
                        safe(quote.quote),
                        false,
                    );
                    if let Some(name) = quote.attrib_name {
                        embed.field("Name", safe(name), false);
                    }
                    if let Some(date) = quote.attrib_date {
                        embed.field("Date", safe(date), false);
                    }
                    if let Some(context) = quote.context {
                        embed.field("Context", safe(context), false);
                    }
                    if let Some(game) = game {
                        embed.field("Game ID", safe(game.id), false).field(
                            "Game name",
                            safe(game.name),
                            false,
                        );
                    }
                    if let Some(game_entry) = game_entry {
                        if let Some(display_name) = game_entry.display_name {
                            embed.field("Game display name", safe(display_name), false);
                        }
                    }
                    if let Some(show) = show {
                        embed.field("Show ID", safe(show.id), false).field(
                            "Show name",
                            safe(show.name),
                            false,
                        );
                    }
                    embed
                })
            })
            .await?;
    } else {
        msg.reply(&ctx, format!("Could not find quote #{}", quote_id)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::{as_ilike, parser::QueryParser, unescape, Column, Expr, Op};
    use std::borrow::Cow;

    #[test]
    fn parsing() {
        let parser = QueryParser::new();
        assert_eq!(parser.parse("butts").unwrap(), Expr::Bare(Cow::Borrowed("butts")));
        assert_eq!(
            parser.parse("bare words get concatenated").unwrap(),
            Expr::Bare(Cow::Borrowed("bare words get concatenated"))
        );
        assert_eq!(
            parser.parse("quote:also quote:FTS quote:fields").unwrap(),
            Expr::Column {
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
