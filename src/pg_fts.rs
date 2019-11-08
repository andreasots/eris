use diesel::expression::SqlLiteral;
use diesel::sql_types::Text;
use diesel::SqlType;
use diesel_full_text_search::{TsQuery, TsVector};

#[derive(SqlType)]
#[postgres(type_name = "regconfig")]
pub struct Regconfig;

// FIXME: there should be a way to do this without SQL literals. It doesn't matter for now because
//  the queries we currently use this in are uncacheable anyway.
pub fn english() -> SqlLiteral<Regconfig> {
    diesel::dsl::sql("'english'::regconfig")
}

sql_function!(fn plainto_tsquery(config: Regconfig, querytext: Text) -> TsQuery);
sql_function!(fn to_tsvector(config: Regconfig, document: Text) -> TsVector);
