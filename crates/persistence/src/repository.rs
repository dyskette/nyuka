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

/// Encodes a string list for a `jsonb` column.
fn json(values: &[String]) -> Result<serde_json::Value> {
    serde_json::to_value(values).map_err(|e| DomainError::Internal(e.to_string()))
}

/// Decodes a `jsonb` string list, treating malformed content as empty.
///
/// A tag list that will not parse is not a reason to make the series
/// unreadable, and the column is written only by this crate.
fn json_list(raw: serde_json::Value) -> Vec<String> {
    serde_json::from_value(raw).unwrap_or_default()
}

fn parse_cursor(cursor: Option<&Cursor>, what: &str) -> Result<Option<Uuid>> {
    cursor
        .map(|c| c.0.parse::<Uuid>())
        .transpose()
        .map_err(|_| DomainError::Invalid(format!("cursor is not a {what} id")))
}

/// Turns a fetched-one-extra row set into a page.
///
/// The extra row is what tells "there is more" from "exactly a full page",
/// which a `LIMIT` alone cannot distinguish.
fn page_of<T>(
    mut rows: Vec<sea_orm::QueryResult>,
    map: impl Fn(&sea_orm::QueryResult) -> Result<T>,
    id_of: impl Fn(&T) -> Uuid,
) -> Result<Page<T>> {
    let has_more = rows.len() as u64 > PAGE_SIZE;
    rows.truncate(PAGE_SIZE as usize);
    let items: Vec<T> = rows.iter().map(map).collect::<Result<_>>()?;
    let next = has_more
        .then(|| items.last().map(|i| Cursor(id_of(i).to_string())))
        .flatten();
    Ok(Page { items, next })
}

fn row_to_manga(row: &sea_orm::QueryResult) -> Result<Manga> {
    Ok(Manga {
        id: MangaId(row.try_get("", "id").map_err(db)?),
        source_id: SourceId(row.try_get("", "source_id").map_err(db)?),
        external_key: ExternalKey(row.try_get("", "external_key").map_err(db)?),
        title: row.try_get("", "title").map_err(db)?,
        authors: json_list(row.try_get("", "authors").map_err(db)?),
        artists: json_list(row.try_get("", "artists").map_err(db)?),
        description: row.try_get("", "description").map_err(db)?,
        tags: json_list(row.try_get("", "tags").map_err(db)?),
        cover_url: row.try_get("", "cover_url").map_err(db)?,
        url: row.try_get("", "url").map_err(db)?,
        language: row.try_get("", "language").map_err(db)?,
        status: MangaStatus::from_i16(row.try_get("", "status").map_err(db)?),
        content_rating: ContentRating::from_i16(row.try_get("", "content_rating").map_err(db)?),
        direction: ReadingDirection::from_i16(row.try_get("", "direction").map_err(db)?),
        created_at: row.try_get("", "created_at").map_err(db)?,
        updated_at: row.try_get("", "updated_at").map_err(db)?,
    })
}

fn row_to_chapter(row: &sea_orm::QueryResult) -> Result<Chapter> {
    Ok(Chapter {
        id: ChapterId(row.try_get("", "id").map_err(db)?),
        manga_id: MangaId(row.try_get("", "manga_id").map_err(db)?),
        external_key: ExternalKey(row.try_get("", "external_key").map_err(db)?),
        title: row.try_get("", "title").map_err(db)?,
        number: row.try_get("", "number").map_err(db)?,
        volume: row.try_get("", "volume").map_err(db)?,
        language: row.try_get("", "language").map_err(db)?,
        published_at: row.try_get("", "published_at").map_err(db)?,
    })
}

fn row_to_source(row: &sea_orm::QueryResult) -> Result<InstalledSource> {
    let version: i32 = row.try_get("", "version").map_err(db)?;
    Ok(InstalledSource {
        id: SourceId(row.try_get("", "id").map_err(db)?),
        repo_id: SourceRepoId(row.try_get("", "repo_id").map_err(db)?),
        external_id: ExternalKey(row.try_get("", "external_id").map_err(db)?),
        name: row.try_get("", "name").map_err(db)?,
        // The column is signed because Postgres has no unsigned integer; a
        // negative value could only come from outside this crate.
        version: version.max(0) as u32,
        languages: json_list(row.try_get("", "languages").map_err(db)?),
        required_capabilities: serde_json::from_value(
            row.try_get("", "required_capabilities").map_err(db)?,
        )
        // A capability this build does not know about must not make the
        // source unreadable: it is re-checked against the package at install
        // time anyway.
        .unwrap_or_default(),
        declared_rate_limit: row
            .try_get::<Option<serde_json::Value>>("", "declared_rate_limit")
            .map_err(db)?
            .and_then(|v| serde_json::from_value(v).ok()),
    })
}

fn row_to_source_entry(row: &sea_orm::QueryResult) -> Result<SourceEntry> {
    let version: i32 = row.try_get("", "version").map_err(db)?;
    Ok(SourceEntry {
        repo_id: SourceRepoId(row.try_get("", "repo_id").map_err(db)?),
        external_id: ExternalKey(row.try_get("", "external_id").map_err(db)?),
        name: row.try_get("", "name").map_err(db)?,
        version: version.max(0) as u32,
        icon_url: row.try_get("", "icon_url").map_err(db)?,
        download_url: row.try_get("", "download_url").map_err(db)?,
        languages: json_list(row.try_get("", "languages").map_err(db)?),
        content_rating: ContentRating::from_i16(row.try_get("", "content_rating").map_err(db)?),
        base_url: row.try_get("", "base_url").map_err(db)?,
        min_app_version: row.try_get("", "min_app_version").map_err(db)?,
    })
}

fn row_to_source_repo(row: &sea_orm::QueryResult) -> Result<SourceRepo> {
    Ok(SourceRepo {
        id: SourceRepoId(row.try_get("", "id").map_err(db)?),
        name: row.try_get("", "name").map_err(db)?,
        url: row.try_get("", "url").map_err(db)?,
        last_refreshed_at: row.try_get("", "last_refreshed_at").map_err(db)?,
        created_at: row.try_get("", "created_at").map_err(db)?,
    })
}

fn row_to_user(row: &sea_orm::QueryResult) -> Result<User> {
    Ok(User {
        id: UserId(row.try_get("", "id").map_err(db)?),
        issuer: row.try_get("", "issuer").map_err(db)?,
        subject: row.try_get("", "subject").map_err(db)?,
        created_at: row.try_get("", "created_at").map_err(db)?,
        last_seen_at: row.try_get("", "last_seen_at").map_err(db)?,
    })
}

fn row_to_follow(row: &sea_orm::QueryResult) -> Result<Follow> {
    Ok(Follow {
        id: FollowId(row.try_get("", "id").map_err(db)?),
        manga_id: MangaId(row.try_get("", "manga_id").map_err(db)?),
        check_interval_secs: row.try_get("", "check_interval_secs").map_err(db)?,
        last_checked_at: row.try_get("", "last_checked_at").map_err(db)?,
        auto_download: row.try_get("", "auto_download").map_err(db)?,
        created_at: row.try_get("", "created_at").map_err(db)?,
    })
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

    // --- manga --------------------------------------------------------------

    /// Reads one series.
    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "manga",
        )
    )]
    pub async fn get_manga(&self, id: MangaId) -> Result<Manga> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM manga WHERE id = $1",
                [id.0.into()],
            ))
            .await
            .map_err(db)?
            .ok_or(DomainError::NotFound)?;
        row_to_manga(&row)
    }

    /// Finds a series by the key its source knows it as.
    ///
    /// The pair is what identity means here: two sources can use the same key
    /// for different series, so neither half alone is enough.
    #[tracing::instrument(
        skip(self, key),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "manga",
        )
    )]
    pub async fn find_manga_by_external(
        &self,
        source: SourceId,
        key: &ExternalKey,
    ) -> Result<Option<Manga>> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM manga WHERE source_id = $1 AND external_key = $2",
                [source.0.into(), key.0.clone().into()],
            ))
            .await
            .map_err(db)?;
        row.as_ref().map(row_to_manga).transpose()
    }

    /// Inserts or updates a series, keyed on `(source_id, external_key)`.
    ///
    /// `created_at` is never overwritten on conflict: a metadata refresh must
    /// not make a series look newly added, which is what the library's "recently
    /// added" ordering reads.
    #[tracing::instrument(
        skip(self, manga),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "INSERT",
            db.collection.name = "manga",
        )
    )]
    pub async fn upsert_manga(&self, manga: &Manga) -> Result<MangaId> {
        let sql = r#"
            INSERT INTO manga (id, source_id, external_key, title, authors, artists,
                               description, tags, cover_url, url, language, status,
                               content_rating, direction, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, now(), now())
            ON CONFLICT (source_id, external_key) DO UPDATE SET
                title = EXCLUDED.title,
                authors = EXCLUDED.authors,
                artists = EXCLUDED.artists,
                description = EXCLUDED.description,
                tags = EXCLUDED.tags,
                cover_url = EXCLUDED.cover_url,
                url = EXCLUDED.url,
                language = EXCLUDED.language,
                status = EXCLUDED.status,
                content_rating = EXCLUDED.content_rating,
                direction = EXCLUDED.direction,
                updated_at = now()
            RETURNING id
        "#;
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                sql,
                [
                    // A fresh id is only used when this is an insert; on
                    // conflict the existing row keeps its own.
                    Uuid::new_v4().into(),
                    manga.source_id.0.into(),
                    manga.external_key.0.clone().into(),
                    manga.title.clone().into(),
                    json(&manga.authors)?.into(),
                    json(&manga.artists)?.into(),
                    manga.description.clone().into(),
                    json(&manga.tags)?.into(),
                    manga.cover_url.clone().into(),
                    manga.url.clone().into(),
                    manga.language.clone().into(),
                    manga.status.as_i16().into(),
                    manga.content_rating.as_i16().into(),
                    manga.direction.as_i16().into(),
                ],
            ))
            .await
            .map_err(db)?
            .ok_or_else(|| DomainError::Internal("upsert returned no row".into()))?;
        Ok(MangaId(row.try_get("", "id").map_err(db)?))
    }

    /// Lists the library, newest first.
    #[tracing::instrument(
        skip(self, cursor),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "manga",
        )
    )]
    pub async fn list_manga(&self, cursor: Option<&Cursor>) -> Result<Page<Manga>> {
        let after = parse_cursor(cursor, "manga")?;
        let (sql, values): (&str, Vec<Value>) = match after {
            Some(id) => (
                "SELECT * FROM manga WHERE (created_at, id) < \
                 (SELECT created_at, id FROM manga WHERE id = $1) \
                 ORDER BY created_at DESC, id DESC LIMIT $2",
                vec![id.into(), ((PAGE_SIZE + 1) as i64).into()],
            ),
            None => (
                "SELECT * FROM manga ORDER BY created_at DESC, id DESC LIMIT $1",
                vec![((PAGE_SIZE + 1) as i64).into()],
            ),
        };
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                sql,
                values,
            ))
            .await
            .map_err(db)?;
        page_of(rows, row_to_manga, |m| m.id.0)
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

        let items: Vec<Chapter> = rows.iter().map(row_to_chapter).collect::<Result<_>>()?;

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

    /// Reads one chapter.
    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "chapter",
        )
    )]
    pub async fn get_chapter(&self, id: ChapterId) -> Result<Chapter> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM chapter WHERE id = $1",
                [id.0.into()],
            ))
            .await
            .map_err(db)?
            .ok_or(DomainError::NotFound)?;
        row_to_chapter(&row)
    }

    // --- users --------------------------------------------------------------

    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "app_user",
        )
    )]
    pub async fn get_user(&self, id: UserId) -> Result<User> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM app_user WHERE id = $1",
                [id.0.into()],
            ))
            .await
            .map_err(db)?
            .ok_or(DomainError::NotFound)?;
        row_to_user(&row)
    }

    /// Records a sign-in, creating the user on first sight.
    ///
    /// One statement rather than select-then-insert: two sign-ins racing on a
    /// first login would otherwise both see no row and both insert.
    ///
    /// The issuer and subject are not logged. They are the person's identity
    /// at their provider, and the observability design records `user.id` —
    /// this row's id, which means nothing outside this database — precisely so
    /// that the subject does not end up in the log stream (ADR-0015).
    #[tracing::instrument(
        skip(self, issuer, subject),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "INSERT",
            db.collection.name = "app_user",
        )
    )]
    pub async fn record_sign_in(&self, issuer: &str, subject: &str) -> Result<User> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "INSERT INTO app_user (id, issuer, subject, created_at, last_seen_at) \
                 VALUES ($1, $2, $3, now(), now()) \
                 ON CONFLICT (issuer, subject) DO UPDATE SET last_seen_at = now() \
                 RETURNING *",
                [Uuid::new_v4().into(), issuer.into(), subject.into()],
            ))
            .await
            .map_err(db)?
            .ok_or_else(|| DomainError::Internal("sign-in returned no row".into()))?;
        row_to_user(&row)
    }

    // --- follows ------------------------------------------------------------

    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "follow",
        )
    )]
    pub async fn get_follow(&self, id: FollowId) -> Result<Follow> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM follow WHERE id = $1",
                [id.0.into()],
            ))
            .await
            .map_err(db)?
            .ok_or(DomainError::NotFound)?;
        row_to_follow(&row)
    }

    #[tracing::instrument(
        skip(self, cursor),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "follow",
        )
    )]
    pub async fn list_follows(&self, cursor: Option<&Cursor>) -> Result<Page<Follow>> {
        let after = parse_cursor(cursor, "follow")?;
        let (sql, values): (&str, Vec<Value>) = match after {
            Some(id) => (
                "SELECT * FROM follow WHERE (created_at, id) < \
                 (SELECT created_at, id FROM follow WHERE id = $1) \
                 ORDER BY created_at DESC, id DESC LIMIT $2",
                vec![id.into(), ((PAGE_SIZE + 1) as i64).into()],
            ),
            None => (
                "SELECT * FROM follow ORDER BY created_at DESC, id DESC LIMIT $1",
                vec![((PAGE_SIZE + 1) as i64).into()],
            ),
        };
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                sql,
                values,
            ))
            .await
            .map_err(db)?;
        page_of(rows, row_to_follow, |f| f.id.0)
    }

    /// Follows whose check interval has elapsed.
    ///
    /// The interval is compared in SQL rather than in application code so the
    /// database does the filtering; loading every follow to find the due ones
    /// would scale with the library instead of with the work.
    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "follow",
        )
    )]
    pub async fn due_follows(&self, limit: u64) -> Result<Vec<Follow>> {
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM follow \
                 WHERE last_checked_at IS NULL \
                    OR last_checked_at + make_interval(secs => check_interval_secs) <= now() \
                 ORDER BY last_checked_at NULLS FIRST \
                 LIMIT $1",
                [(limit as i64).into()],
            ))
            .await
            .map_err(db)?;
        rows.iter().map(row_to_follow).collect()
    }

    /// Inserts or updates a follow. One follow per series, by the unique key
    /// on `manga_id`.
    #[tracing::instrument(
        skip(self, follow),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "INSERT",
            db.collection.name = "follow",
        )
    )]
    pub async fn upsert_follow(&self, follow: &Follow) -> Result<FollowId> {
        let sql = r#"
            INSERT INTO follow (id, manga_id, check_interval_secs, last_checked_at,
                                auto_download, created_at)
            VALUES ($1, $2, $3, $4, $5, now())
            ON CONFLICT (manga_id) DO UPDATE SET
                check_interval_secs = EXCLUDED.check_interval_secs,
                auto_download = EXCLUDED.auto_download
            RETURNING id
        "#;
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                sql,
                [
                    Uuid::new_v4().into(),
                    follow.manga_id.0.into(),
                    follow.check_interval_secs.into(),
                    follow
                        .last_checked_at
                        .map(Value::from)
                        .unwrap_or(Value::ChronoDateTimeUtc(None)),
                    follow.auto_download.into(),
                ],
            ))
            .await
            .map_err(db)?
            .ok_or_else(|| DomainError::Internal("upsert returned no row".into()))?;
        Ok(FollowId(row.try_get("", "id").map_err(db)?))
    }

    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "DELETE",
            db.collection.name = "follow",
        )
    )]
    pub async fn delete_follow(&self, id: FollowId) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "DELETE FROM follow WHERE id = $1",
                [id.0.into()],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Stamps a successful check.
    ///
    /// Only the handler calls this, and only on success: stamping on enqueue
    /// would make a failed check wait out a whole interval before trying
    /// again.
    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "UPDATE",
            db.collection.name = "follow",
        )
    )]
    pub async fn mark_follow_checked(&self, id: FollowId) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "UPDATE follow SET last_checked_at = now() WHERE id = $1",
                [id.0.into()],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    // --- sources and repositories -------------------------------------------

    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "source",
        )
    )]
    pub async fn get_source(&self, id: SourceId) -> Result<InstalledSource> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM source WHERE id = $1",
                [id.0.into()],
            ))
            .await
            .map_err(db)?
            .ok_or(DomainError::NotFound)?;
        row_to_source(&row)
    }

    /// Every installed source.
    ///
    /// Not paginated: the count is bounded by what an operator installed, and
    /// the job engine needs all of them to register rate limits at startup.
    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "source",
        )
    )]
    pub async fn list_sources(&self) -> Result<Vec<InstalledSource>> {
        let rows = self
            .db
            .query_all_raw(Statement::from_string(
                self.db.get_database_backend(),
                "SELECT * FROM source ORDER BY name",
            ))
            .await
            .map_err(db)?;
        rows.iter().map(row_to_source).collect()
    }

    /// Inserts or updates a source, keyed on `(repo_id, external_id)`.
    ///
    /// Reinstalling keeps the row's id, so every `manga` row pointing at it
    /// survives an upgrade. A new id would orphan the whole library for that
    /// source.
    #[tracing::instrument(
        skip(self, source),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "INSERT",
            db.collection.name = "source",
        )
    )]
    pub async fn upsert_source(&self, source: &InstalledSource) -> Result<SourceId> {
        let rate_limit = source
            .declared_rate_limit
            .map(|r| serde_json::to_value(r).map_err(|e| DomainError::Internal(e.to_string())))
            .transpose()?;
        let sql = r#"
            INSERT INTO source (id, repo_id, external_id, name, version, languages,
                                required_capabilities, declared_rate_limit, installed_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
            ON CONFLICT (repo_id, external_id) DO UPDATE SET
                name = EXCLUDED.name,
                version = EXCLUDED.version,
                languages = EXCLUDED.languages,
                required_capabilities = EXCLUDED.required_capabilities,
                declared_rate_limit = COALESCE(EXCLUDED.declared_rate_limit,
                                               source.declared_rate_limit)
            RETURNING id
        "#;
        let capabilities = serde_json::to_value(&source.required_capabilities)
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                sql,
                [
                    Uuid::new_v4().into(),
                    source.repo_id.0.into(),
                    source.external_id.0.clone().into(),
                    source.name.clone().into(),
                    (source.version as i32).into(),
                    json(&source.languages)?.into(),
                    capabilities.into(),
                    rate_limit.map(Value::from).unwrap_or(Value::Json(None)),
                ],
            ))
            .await
            .map_err(db)?
            .ok_or_else(|| DomainError::Internal("upsert returned no row".into()))?;
        Ok(SourceId(row.try_get("", "id").map_err(db)?))
    }

    /// Uninstalls a source.
    ///
    /// > [!WARNING]
    /// > The foreign keys cascade, so this deletes every `manga` row from this
    /// > source and everything hanging off them. Packaged files in the library
    /// > are untouched — they are the user's, and nothing else knows how to
    /// > find them again.
    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "DELETE",
            db.collection.name = "source",
        )
    )]
    pub async fn delete_source(&self, id: SourceId) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "DELETE FROM source WHERE id = $1",
                [id.0.into()],
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
            db.collection.name = "source_repo",
        )
    )]
    pub async fn list_source_repos(&self) -> Result<Vec<SourceRepo>> {
        let rows = self
            .db
            .query_all_raw(Statement::from_string(
                self.db.get_database_backend(),
                "SELECT * FROM source_repo ORDER BY created_at",
            ))
            .await
            .map_err(db)?;
        rows.iter().map(row_to_source_repo).collect()
    }

    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "source_repo",
        )
    )]
    pub async fn get_source_repo(&self, id: SourceRepoId) -> Result<SourceRepo> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM source_repo WHERE id = $1",
                [id.0.into()],
            ))
            .await
            .map_err(db)?
            .ok_or(DomainError::NotFound)?;
        row_to_source_repo(&row)
    }

    /// Adds or renames a repository, keyed on its url.
    ///
    /// `last_refreshed_at` is not touched: adding the same url twice must not
    /// make an index that was never fetched look current.
    #[tracing::instrument(
        skip(self, repo),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "INSERT",
            db.collection.name = "source_repo",
        )
    )]
    pub async fn upsert_source_repo(&self, repo: &SourceRepo) -> Result<SourceRepoId> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "INSERT INTO source_repo (id, name, url, created_at) \
                 VALUES ($1, $2, $3, now()) \
                 ON CONFLICT (url) DO UPDATE SET name = EXCLUDED.name \
                 RETURNING id",
                [
                    Uuid::new_v4().into(),
                    repo.name.clone().into(),
                    repo.url.clone().into(),
                ],
            ))
            .await
            .map_err(db)?
            .ok_or_else(|| DomainError::Internal("upsert returned no row".into()))?;
        Ok(SourceRepoId(row.try_get("", "id").map_err(db)?))
    }

    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "DELETE",
            db.collection.name = "source_repo",
        )
    )]
    pub async fn delete_source_repo(&self, id: SourceRepoId) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "DELETE FROM source_repo WHERE id = $1",
                [id.0.into()],
            ))
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Stamps a successful index refresh, so a failed one leaves the
    /// repository looking as stale as it is.
    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "UPDATE",
            db.collection.name = "source_repo",
        )
    )]
    pub async fn mark_repo_refreshed(&self, id: SourceRepoId) -> Result<()> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "UPDATE source_repo SET last_refreshed_at = now() WHERE id = $1",
                [id.0.into()],
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
            db.collection.name = "source_repo_entry",
        )
    )]
    pub async fn list_repo_entries(&self, repo: SourceRepoId) -> Result<Vec<SourceEntry>> {
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM source_repo_entry WHERE repo_id = $1 ORDER BY name",
                [repo.0.into()],
            ))
            .await
            .map_err(db)?;
        rows.iter().map(row_to_source_entry).collect()
    }

    #[tracing::instrument(
        skip(self, external),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "source_repo_entry",
        )
    )]
    pub async fn get_repo_entry(
        &self,
        repo: SourceRepoId,
        external: &ExternalKey,
    ) -> Result<SourceEntry> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT * FROM source_repo_entry WHERE repo_id = $1 AND external_id = $2",
                [repo.0.into(), external.0.clone().into()],
            ))
            .await
            .map_err(db)?
            .ok_or(DomainError::NotFound)?;
        row_to_source_entry(&row)
    }

    /// Replaces a repository's catalog in one transaction.
    ///
    /// Delete-then-insert rather than upsert-and-prune: an index is a document
    /// that is either current or not, and reconciling row by row would leave
    /// the catalog mixed if the pass failed partway. The transaction is what
    /// makes "either the old index or the new one, never half of each" true.
    #[tracing::instrument(
        skip(self, entries),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "INSERT",
            db.collection.name = "source_repo_entry",
            entry.count = entries.len(),
        )
    )]
    pub async fn replace_repo_entries(
        &self,
        repo: SourceRepoId,
        entries: &[SourceEntry],
    ) -> Result<u64> {
        use sea_orm::TransactionTrait;

        let backend = self.db.get_database_backend();
        let txn = self.db.begin().await.map_err(db)?;

        txn.execute_raw(Statement::from_sql_and_values(
            backend,
            "DELETE FROM source_repo_entry WHERE repo_id = $1",
            [repo.0.into()],
        ))
        .await
        .map_err(db)?;

        for entry in entries {
            txn.execute_raw(Statement::from_sql_and_values(
                backend,
                "INSERT INTO source_repo_entry (repo_id, external_id, name, version, icon_url, \
                 download_url, languages, content_rating, base_url, min_app_version, fetched_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now())",
                [
                    repo.0.into(),
                    entry.external_id.0.clone().into(),
                    entry.name.clone().into(),
                    (entry.version as i32).into(),
                    entry.icon_url.clone().into(),
                    entry.download_url.clone().into(),
                    json(&entry.languages)?.into(),
                    entry.content_rating.as_i16().into(),
                    entry.base_url.clone().into(),
                    entry.min_app_version.clone().into(),
                ],
            ))
            .await
            .map_err(db)?;
        }

        txn.commit().await.map_err(db)?;
        Ok(entries.len() as u64)
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

    /// Every setting a source has stored.
    #[tracing::instrument(
        skip(self),
        fields(
            db.system.name = "postgresql",
            db.operation.name = "SELECT",
            db.collection.name = "source_kv",
        )
    )]
    pub async fn kv_list(&self, source: SourceId) -> Result<Vec<(String, Vec<u8>)>> {
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                self.db.get_database_backend(),
                "SELECT key, value FROM source_kv WHERE source_id = $1 ORDER BY key",
                [source.0.into()],
            ))
            .await
            .map_err(db)?;

        rows.into_iter()
            .map(|row| {
                Ok((
                    row.try_get("", "key").map_err(db)?,
                    row.try_get("", "value").map_err(db)?,
                ))
            })
            .collect()
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
