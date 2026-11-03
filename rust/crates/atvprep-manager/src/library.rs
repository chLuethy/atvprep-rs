//! Library roots.

use std::path::PathBuf;

use rusqlite::OptionalExtension;

use crate::Result;
use crate::models::LibraryRoot;
use crate::repo::Ctx;

pub struct LibraryRoots<'a>(pub Ctx<'a>);

impl LibraryRoots<'_> {
    /// # Errors
    ///
    /// A root with this name or path exists.
    pub fn create(&self, name: &str, canonical_path: &str) -> Result<LibraryRoot> {
        self.0.conn.execute(
            "INSERT INTO library_roots(name, canonical_path) VALUES (?1, ?2)",
            [name, canonical_path],
        )?;
        let id = self.0.conn.last_insert_rowid();
        self.get(id)?
            .ok_or_else(|| crate::Error::NotFound(format!("unknown root id: {id}")))
    }

    /// # Errors
    ///
    /// SQLite fails.
    pub fn get(&self, root_id: i64) -> Result<Option<LibraryRoot>> {
        Ok(self
            .0
            .conn
            .query_row(
                "SELECT * FROM library_roots WHERE id = ?1",
                [root_id],
                LibraryRoot::from_row,
            )
            .optional()?)
    }

    /// Register the libraries the configuration lists, once, and return how many were new. A fresh
    /// database knows no library, and the web interface cannot add one. Existing rows are left as
    /// they are, including a library someone switched off.
    ///
    /// # Errors
    ///
    /// SQLite fails.
    pub fn ensure_configured(&self, paths: &[PathBuf]) -> Result<usize> {
        let column = |sql: &str| -> Result<std::collections::HashSet<String>> {
            let mut statement = self.0.conn.prepare(sql)?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<std::result::Result<_, _>>()?)
        };
        let mut known = column("SELECT canonical_path FROM library_roots")?;
        let mut names = column("SELECT name FROM library_roots")?;
        let mut created = 0;
        for path in paths {
            let canonical = path.to_string_lossy().into_owned();
            if known.contains(&canonical) {
                continue;
            }
            let base = path
                .file_name()
                .map_or_else(|| canonical.clone(), |n| n.to_string_lossy().into_owned());
            let (mut name, mut number) = (base.clone(), 1);
            while names.contains(&name) {
                number += 1;
                name = format!("{base} ({number})");
            }
            self.create(&name, &canonical)?;
            known.insert(canonical);
            names.insert(name);
            created += 1;
        }
        Ok(created)
    }
}
