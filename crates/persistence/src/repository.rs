//! Repository adapters.
//!
//! Each method is instrumented with the stable database semantic conventions —
//! `db.system.name`, `db.operation.name`, `db.collection.name` — and arguments
//! that may carry large payloads are skipped (ADR-0015).
//!
//! Entities are an implementation detail: nothing here returns one. Rows are
//! mapped to domain types at this boundary, which is what lets the entity
//! files stay generated (ADR-0002).

use nyuka_domain::model::*;
use nyuka_domain::{DomainError, Result};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, Value};
use uuid::Uuid;

fn db(e: sea_orm::DbErr) -> DomainError {
    DomainError::Storage(e.to_string())
}

/// Default page size for cursor-paginated reads.
const PAGE_SIZE: u64 = 50;

#[derive(Debug, Clone)]
pub struct Repositories {
    db: DatabaseConnection,
}

impl Repositories {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    // --- chapters -----------------------------------------------------------

    /// Inserts chapters, returning **only the ones that did not already exist**.
    ///
    /// `ON CONFLICT DO NOTHING ... RETURNING` is what makes this work: Postgres
    /// returns rows it actually inserted, so the result is exactly the new
    /// chapters. Computing the difference in application code would need a
    /// second query and would race a concurrent refresh of the same series.
    ///
    /// This set drives `chapter.new` events and the follows badge (ADR-0010),
    /// so the distinction is not cosmetic: returning every chapter notifies on
    /// every refresh, and returning none notifies never.
    #[tracing::instrument(
        skip(self, chapters),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "INSERT",
            db.collection.name = "chapter",
            chapter.count = chapters.len(),
        )
    )]
    pub async fn upsert_chapters(
        &self,
        manga: MangaId,
        chapters: &[SourceChapter],
    ) -> Result<Vec<ChapterId>> {
        if chapters.is_empty() {
            return Ok(Vec::new());
        }

        let mut sql = String::from(
            "INSERT INTO chapter (id, manga_id, external_key, title, number, volume, \
             published_at, scanlators, url, language, locked, created_at) VALUES ",
        );
        let mut values: Vec<Value> = Vec::new();
        for (i, c) in chapters.iter().enumerate() {
            let b = i * 11;
            if i > 0 {
                sql.push_str(", ");
            }
            sql.push_str(&format!(
                "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, now())",
                b + 1,
                b + 2,
                b + 3,
                b + 4,
                b + 5,
                b + 6,
                b + 7,
                b + 8,
                b + 9,
                b + 10,
                b + 11
            ));
            values.extend([
                Uuid::new_v4().into(),
                manga.0.into(),
                c.key.0.clone().into(),
                c.title
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::String(None)),
                c.number.map(Value::from).unwrap_or(Value::Float(None)),
                c.volume.map(Value::from).unwrap_or(Value::Float(None)),
                c.published_at
                    .map(Value::from)
                    .unwrap_or(Value::ChronoDateTimeUtc(None)),
                serde_json::to_value(&c.scanlators)
                    .unwrap_or_default()
                    .into(),
                c.url
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::String(None)),
                c.language
                    .clone()
                    .map(Value::from)
                    .unwrap_or(Value::String(None)),
                c.locked.into(),
            ]);
        }
        // The unique index on (manga_id, external_key) is what identifies a
        // chapter, and DO NOTHING means only genuinely new rows come back.
        sql.push_str(" ON CONFLICT (manga_id, external_key) DO NOTHING RETURNING id");

        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                &sql,
                values,
            ))
            .await
            .map_err(db)?;

        rows.iter()
            .map(|r| Ok(ChapterId(r.try_get("", "id").map_err(db)?)))
            .collect()
    }

    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "chapter",
        )
    )]
    pub async fn list_chapters(
        &self,
        manga: MangaId,
        cursor: Option<&Cursor>,
    ) -> Result<Page<Chapter>> {
        // Keyset pagination on (number, id): stable under inserts, unlike an
        // offset, which shifts every page when a new chapter lands.
        let after: Option<Uuid> = cursor
            .map(|c| c.0.parse::<Uuid>())
            .transpose()
            .map_err(|_| DomainError::Invalid("cursor is not a chapter id".into()))?;

        let (sql, values): (&str, Vec<Value>) = match after {
            Some(id) => (
                "SELECT * FROM chapter WHERE manga_id = $1 AND (number, id) > \
                 (SELECT COALESCE(number, 0), id FROM chapter WHERE id = $2) \
                 ORDER BY number NULLS FIRST, id LIMIT $3",
                vec![manga.0.into(), id.into(), ((PAGE_SIZE + 1) as i64).into()],
            ),
            None => (
                "SELECT * FROM chapter WHERE manga_id = $1 \
                 ORDER BY number NULLS FIRST, id LIMIT $2",
                vec![manga.0.into(), ((PAGE_SIZE + 1) as i64).into()],
            ),
        };

        let mut rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                sql,
                values,
            ))
            .await
            .map_err(db)?;

        // One extra row is fetched to tell "there is more" from "exactly a full
        // page", which a LIMIT alone cannot distinguish.
        let has_more = rows.len() as u64 > PAGE_SIZE;
        rows.truncate(PAGE_SIZE as usize);

        let items: Vec<Chapter> = rows
            .iter()
            .map(|r| {
                Ok(Chapter {
                    id: ChapterId(r.try_get("", "id").map_err(db)?),
                    manga_id: MangaId(r.try_get("", "manga_id").map_err(db)?),
                    external_key: ExternalKey(r.try_get("", "external_key").map_err(db)?),
                    title: r.try_get("", "title").map_err(db)?,
                    number: r.try_get("", "number").map_err(db)?,
                    volume: r.try_get("", "volume").map_err(db)?,
                    language: r.try_get("", "language").map_err(db)?,
                    published_at: r.try_get("", "published_at").map_err(db)?,
                })
            })
            .collect::<Result<_>>()?;

        let next = has_more
            .then(|| items.last().map(|c| Cursor(c.id.0.to_string())))
            .flatten();
        Ok(Page { items, next })
    }

    /// Records a packaged chapter.
    #[tracing::instrument(
        skip(self, download),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "INSERT",
            db.collection.name = "downloaded_chapter",
        )
    )]
    pub async fn record_download(&self, download: &DownloadedChapter) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "INSERT INTO downloaded_chapter (chapter_id, relative_path, size_bytes, \
                 checksum, packaged_at) VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (chapter_id) DO UPDATE SET relative_path = EXCLUDED.relative_path, \
                 size_bytes = EXCLUDED.size_bytes, checksum = EXCLUDED.checksum, \
                 packaged_at = EXCLUDED.packaged_at",
                [
                    download.chapter_id.0.into(),
                    download.relative_path.clone().into(),
                    (download.size_bytes as i64).into(),
                    download.checksum.clone().into(),
                    download.packaged_at.into(),
                ],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "downloaded_chapter",
        )
    )]
    pub async fn downloaded(&self, chapter: ChapterId) -> Result<Option<DownloadedChapter>> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM downloaded_chapter WHERE chapter_id = $1",
                [chapter.0.into()],
            ))
            .await
            .map_err(db)?;
        let Some(r) = row else { return Ok(None) };
        let size: i64 = r.try_get("", "size_bytes").map_err(db)?;
        Ok(Some(DownloadedChapter {
            chapter_id: ChapterId(r.try_get("", "chapter_id").map_err(db)?),
            relative_path: r.try_get("", "relative_path").map_err(db)?,
            size_bytes: size as u64,
            checksum: r.try_get("", "checksum").map_err(db)?,
            packaged_at: r.try_get("", "packaged_at").map_err(db)?,
        }))
    }

    /// Marks rows whose files have gone missing.
    ///
    /// The `reconcile_library` job kind. ADR-0007 notes the asymmetry that
    /// makes this necessary: `pg_dump` protects metadata but not the library
    /// volume, so a disk failure leaves the database referencing files that no
    /// longer exist.
    #[tracing::instrument(
        skip(self, missing),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "DELETE",
            db.collection.name = "downloaded_chapter",
        )
    )]
    pub async fn forget_downloads(&self, missing: &[ChapterId]) -> Result<u64> {
        if missing.is_empty() {
            return Ok(0);
        }
        let ids: Vec<Uuid> = missing.iter().map(|c| c.0).collect();
        let result = self
            .db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "DELETE FROM downloaded_chapter WHERE chapter_id = ANY($1)",
                [ids.into()],
            ))
            .await
            .map_err(db)?;
        Ok(result.rows_affected())
    }

    // --- source key-value ---------------------------------------------------

    /// Reads a source's stored setting.
    ///
    /// Values are opaque bytes: the WASM `defaults` import postcard-encodes
    /// them, and this layer does not interpret them (ADR-0004).
    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "source_kv",
        )
    )]
    pub async fn kv_get(&self, source: SourceId, key: &str) -> Result<Option<Vec<u8>>> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT value FROM source_kv WHERE source_id = $1 AND key = $2",
                [source.0.into(), key.into()],
            ))
            .await
            .map_err(db)?;
        row.map(|r| r.try_get("", "value").map_err(db)).transpose()
    }

    #[tracing::instrument(
        skip(self, value),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "INSERT",
            db.collection.name = "source_kv",
        )
    )]
    pub async fn kv_set(&self, source: SourceId, key: &str, value: Vec<u8>) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "INSERT INTO source_kv (source_id, key, value, updated_at) \
                 VALUES ($1, $2, $3, now()) \
                 ON CONFLICT (source_id, key) DO UPDATE SET value = EXCLUDED.value, \
                 updated_at = now()",
                [source.0.into(), key.into(), value.into()],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }
}
