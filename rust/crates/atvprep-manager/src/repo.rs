//! What every repository needs: a connection (usually inside a transaction) and a clock.

use rusqlite::Connection;

use crate::clock::Clock;

#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub conn: &'a Connection,
    pub clock: &'a dyn Clock,
}
