use crate::config::Config;
use crate::rpc::server::Channel;
use crate::rpc::LRRbot;
use crate::PgPool;

async fn stream_up_inner(lrrbot: &mut LRRbot, pg_pool: PgPool, data: Channel) -> Result<(), Error> {
    let (game_id, show_id) = join!(lrrbot.get_game_id(), lrrbot.get_show_id());
    let game_id = game_id.context("failed to get the game ID")?;
    let show_id = show_id.context("failed to get the show ID")?;

    let conn = pg_pool
            .get()
            .context("failed to get a database connection from the pool")?;

        let game = match header.current_game {
            Some(game) => {
                use crate::schema::games::dsl::*;

                Some(
                    games
                        .find(game.id)
                        .first::<Game>(&conn)
                        .context("failed to load the game")?,
                )
            }
            None => None,
        };

        let show = match header.current_show {
            Some(show) => {
                use crate::schema::shows::dsl::*;

                Some(
                    shows
                        .find(show.id)
                        .first::<Show>(&conn)
                        .context("failed to load the show")?,
                )
            }
            None => None,
        };

        let game_entry = match (header.current_game, header.current_show) {
            (Some(game), Some(show)) => {
                use crate::schema::game_per_show_data::dsl::*;

                game_per_show_data
                    .find((game.id, show.id))
                    .first::<GameEntry>(&conn)
                    .optional()
                    .context("failed to load the game entry")?
            }
            _ => None,
        };
}

pub async fn stream_up(config: &Config, pg_pool: PgPool, data: Channel) {
    let mut lrrbot = LRRbot::new(config);

    match await!(stream_up_inner(&mut lrrbot, pg_pool, data)) {
        Ok(()) => (),
        Err(err) => eprintln!("failed to post a stream up announcement: {:?}", err),
    }
}
