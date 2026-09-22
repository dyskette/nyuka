//! The initial schema.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // --- sources ---------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(SourceRepo::Table)
                    .if_not_exists()
                    .col(pk_uuid(SourceRepo::Id))
                    .col(string(SourceRepo::Name))
                    .col(string(SourceRepo::Url).unique_key())
                    .col(timestamp_with_time_zone_null(SourceRepo::LastRefreshedAt))
                    .col(timestamp_with_time_zone(SourceRepo::CreatedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Source::Table)
                    .if_not_exists()
                    .col(pk_uuid(Source::Id))
                    .col(uuid(Source::RepoId))
                    // The source's own id, such as "en.asurascans".
                    .col(string(Source::ExternalId))
                    .col(string(Source::Name))
                    .col(integer(Source::Version))
                    .col(json_binary(Source::Languages))
                    .col(json_binary(Source::RequiredCapabilities))
                    .col(json_binary_null(Source::DeclaredRateLimit))
                    .col(timestamp_with_time_zone(Source::InstalledAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Source::Table, Source::RepoId)
                            .to(SourceRepo::Table, SourceRepo::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_source_repo_external")
                    .table(Source::Table)
                    .col(Source::RepoId)
                    .col(Source::ExternalId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // The per-source key-value namespace the WASM `defaults` import reads
        // and writes.
        manager
            .create_table(
                Table::create()
                    .table(SourceKv::Table)
                    .if_not_exists()
                    .col(uuid(SourceKv::SourceId))
                    .col(string(SourceKv::Key))
                    .col(binary(SourceKv::Value))
                    .col(timestamp_with_time_zone(SourceKv::UpdatedAt))
                    .primary_key(Index::create().col(SourceKv::SourceId).col(SourceKv::Key))
                    .foreign_key(
                        ForeignKey::create()
                            .from(SourceKv::Table, SourceKv::SourceId)
                            .to(Source::Table, Source::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // --- library ---------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Manga::Table)
                    .if_not_exists()
                    .col(pk_uuid(Manga::Id))
                    .col(uuid(Manga::SourceId))
                    .col(string(Manga::ExternalKey))
                    .col(string(Manga::Title))
                    .col(json_binary(Manga::Authors))
                    .col(json_binary(Manga::Artists))
                    .col(text_null(Manga::Description))
                    .col(json_binary(Manga::Tags))
                    .col(string_null(Manga::CoverUrl))
                    .col(string_null(Manga::Url))
                    .col(string_null(Manga::Language))
                    .col(small_integer(Manga::Status))
                    .col(small_integer(Manga::ContentRating))
                    .col(small_integer(Manga::Direction))
                    .col(timestamp_with_time_zone(Manga::CreatedAt))
                    .col(timestamp_with_time_zone(Manga::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Manga::Table, Manga::SourceId)
                            .to(Source::Table, Source::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // A series is identified by its source plus that source's own key.
        manager
            .create_index(
                Index::create()
                    .name("idx_manga_source_external")
                    .table(Manga::Table)
                    .col(Manga::SourceId)
                    .col(Manga::ExternalKey)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Chapter::Table)
                    .if_not_exists()
                    .col(pk_uuid(Chapter::Id))
                    .col(uuid(Chapter::MangaId))
                    .col(string(Chapter::ExternalKey))
                    .col(string_null(Chapter::Title))
                    .col(float_null(Chapter::Number))
                    .col(float_null(Chapter::Volume))
                    .col(timestamp_with_time_zone_null(Chapter::PublishedAt))
                    .col(json_binary(Chapter::Scanlators))
                    .col(string_null(Chapter::Url))
                    .col(string_null(Chapter::Language))
                    .col(boolean(Chapter::Locked))
                    .col(timestamp_with_time_zone(Chapter::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Chapter::Table, Chapter::MangaId)
                            .to(Manga::Table, Manga::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_chapter_manga_external")
                    .table(Chapter::Table)
                    .col(Chapter::MangaId)
                    .col(Chapter::ExternalKey)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(DownloadedChapter::Table)
                    .if_not_exists()
                    .col(uuid(DownloadedChapter::ChapterId).primary_key())
                    // Relative to the library root: LibraryStore never hands
                    // out absolute paths (ADR-0007).
                    .col(string(DownloadedChapter::RelativePath))
                    .col(big_integer(DownloadedChapter::SizeBytes))
                    .col(string(DownloadedChapter::Checksum))
                    .col(timestamp_with_time_zone(DownloadedChapter::PackagedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(DownloadedChapter::Table, DownloadedChapter::ChapterId)
                            .to(Chapter::Table, Chapter::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Follow::Table)
                    .if_not_exists()
                    .col(pk_uuid(Follow::Id))
                    .col(uuid(Follow::MangaId).unique_key())
                    .col(integer(Follow::CheckIntervalSecs))
                    .col(timestamp_with_time_zone_null(Follow::LastCheckedAt))
                    .col(boolean(Follow::AutoDownload))
                    .col(timestamp_with_time_zone(Follow::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Follow::Table, Follow::MangaId)
                            .to(Manga::Table, Manga::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // --- jobs ------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(Job::Table)
                    .if_not_exists()
                    .col(pk_uuid(Job::Id))
                    .col(string(Job::Kind))
                    .col(json_binary(Job::Payload))
                    .col(string(Job::State))
                    .col(small_integer(Job::Priority))
                    .col(timestamp_with_time_zone(Job::RunAt))
                    .col(integer(Job::Attempts))
                    .col(integer(Job::MaxAttempts))
                    .col(string_null(Job::LockedBy))
                    .col(timestamp_with_time_zone_null(Job::LockedAt))
                    .col(text_null(Job::LastError))
                    // Unique, and enforced in the same database as the domain
                    // writes it accompanies — which is the whole reason the
                    // queue lives here (ADR-0003).
                    .col(string_null(Job::IdempotencyKey))
                    .col(timestamp_with_time_zone(Job::CreatedAt))
                    .col(timestamp_with_time_zone(Job::UpdatedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_job_idempotency_key")
                    .table(Job::Table)
                    .col(Job::IdempotencyKey)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // --- auth ------------------------------------------------------------
        manager
            .create_table(
                Table::create()
                    .table(AppUser::Table)
                    .if_not_exists()
                    .col(pk_uuid(AppUser::Id))
                    .col(string(AppUser::Issuer))
                    .col(string(AppUser::Subject))
                    .col(timestamp_with_time_zone(AppUser::CreatedAt))
                    .col(timestamp_with_time_zone(AppUser::LastSeenAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_app_user_issuer_subject")
                    .table(AppUser::Table)
                    .col(AppUser::Issuer)
                    .col(AppUser::Subject)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Sessions are hand-written rather than using a tower-sessions store
        // crate: every published one needs SQLx 0.8 or SeaORM 1.1, which do
        // not unify with this workspace (ADR-0005).
        manager
            .create_table(
                Table::create()
                    .table(Session::Table)
                    .if_not_exists()
                    .col(string(Session::Id).primary_key())
                    .col(uuid_null(Session::UserId))
                    .col(binary(Session::Data))
                    .col(timestamp_with_time_zone(Session::ExpiresAt))
                    .foreign_key(
                        ForeignKey::create()
                            .from(Session::Table, Session::UserId)
                            .to(AppUser::Table, AppUser::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_session_expires_at")
                    .table(Session::Table)
                    .col(Session::ExpiresAt)
                    .to_owned(),
            )
            .await?;

        // --- raw SQL: partial index and table tuning -------------------------
        // sea-query cannot express a partial index, and this one is what keeps
        // the claim query fast as terminal rows accumulate (ADR-0003).
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"
            CREATE INDEX IF NOT EXISTS idx_job_claimable
                ON job (priority, run_at)
                WHERE state = 'queued';
            "#,
        )
        .await?;

        // The job table is high-churn: without a lower scale factor autovacuum
        // runs too rarely, the partial index bloats, and claim latency drifts
        // upward (ADR-0003).
        db.execute_unprepared(
            r#"
            ALTER TABLE job SET (
                autovacuum_vacuum_scale_factor = 0.02,
                autovacuum_analyze_scale_factor = 0.01
            );
            "#,
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Reverse dependency order.
        for table in [
            Session::Table.into_iden(),
            AppUser::Table.into_iden(),
            Job::Table.into_iden(),
            Follow::Table.into_iden(),
            DownloadedChapter::Table.into_iden(),
            Chapter::Table.into_iden(),
            Manga::Table.into_iden(),
            SourceKv::Table.into_iden(),
            Source::Table.into_iden(),
            SourceRepo::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).if_exists().to_owned())
                .await?;
        }
        Ok(())
    }
}

#[derive(DeriveIden)]
enum SourceRepo {
    Table,
    Id,
    Name,
    Url,
    LastRefreshedAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Source {
    Table,
    Id,
    RepoId,
    ExternalId,
    Name,
    Version,
    Languages,
    RequiredCapabilities,
    DeclaredRateLimit,
    InstalledAt,
}

#[derive(DeriveIden)]
enum SourceKv {
    Table,
    SourceId,
    Key,
    Value,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Manga {
    Table,
    Id,
    SourceId,
    ExternalKey,
    Title,
    Authors,
    Artists,
    Description,
    Tags,
    CoverUrl,
    Url,
    Language,
    Status,
    ContentRating,
    Direction,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Chapter {
    Table,
    Id,
    MangaId,
    ExternalKey,
    Title,
    Number,
    Volume,
    PublishedAt,
    Scanlators,
    Url,
    Language,
    Locked,
    CreatedAt,
}

#[derive(DeriveIden)]
enum DownloadedChapter {
    Table,
    ChapterId,
    RelativePath,
    SizeBytes,
    Checksum,
    PackagedAt,
}

#[derive(DeriveIden)]
enum Follow {
    Table,
    Id,
    MangaId,
    CheckIntervalSecs,
    LastCheckedAt,
    AutoDownload,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Job {
    Table,
    Id,
    Kind,
    Payload,
    State,
    Priority,
    RunAt,
    Attempts,
    MaxAttempts,
    LockedBy,
    LockedAt,
    LastError,
    IdempotencyKey,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum AppUser {
    Table,
    Id,
    Issuer,
    Subject,
    CreatedAt,
    LastSeenAt,
}

#[derive(DeriveIden)]
enum Session {
    Table,
    Id,
    UserId,
    Data,
    ExpiresAt,
}
