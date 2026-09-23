//! The catalog of sources a repository publishes.
//!
//! `source_repo` records where an index lives; `source` records what is
//! installed. Neither holds what is *available*, which is what a user browses
//! before installing anything. Without this table `update_sources` has nowhere
//! to put what it fetched, and the browse endpoint would have to hit every
//! configured repository on every request.
//!
//! Rows are a cache of a remote document and are replaced wholesale per
//! repository on each refresh, so a source dropped upstream disappears here
//! rather than lingering as an entry whose download URL now 404s.

use sea_orm_migration::prelude::*;
use sea_orm_migration::schema::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(SourceRepoEntry::Table)
                    .if_not_exists()
                    .col(uuid(SourceRepoEntry::RepoId))
                    // The source's own id, such as "en.guya". Unique only
                    // within a repository: two repositories may publish the
                    // same source, and an operator may have both configured.
                    .col(string(SourceRepoEntry::ExternalId))
                    .col(string(SourceRepoEntry::Name))
                    .col(integer(SourceRepoEntry::Version))
                    .col(string_null(SourceRepoEntry::IconUrl))
                    // Stored resolved against the index URL, not as the
                    // relative form the index publishes, so nothing
                    // downstream has to know where the index came from.
                    .col(string(SourceRepoEntry::DownloadUrl))
                    .col(json_binary(SourceRepoEntry::Languages))
                    .col(small_integer(SourceRepoEntry::ContentRating))
                    .col(string_null(SourceRepoEntry::BaseUrl))
                    .col(string_null(SourceRepoEntry::MinAppVersion))
                    .col(timestamp_with_time_zone(SourceRepoEntry::FetchedAt))
                    .primary_key(
                        Index::create()
                            .col(SourceRepoEntry::RepoId)
                            .col(SourceRepoEntry::ExternalId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(SourceRepoEntry::Table, SourceRepoEntry::RepoId)
                            .to(SourceRepo::Table, SourceRepo::Id)
                            // Removing a repository removes what it offered.
                            // Leaving entries behind would show installable
                            // sources from an index nobody is refreshing.
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // The browse listing orders by name within a repository.
        manager
            .create_index(
                Index::create()
                    .name("idx_source_repo_entry_repo_name")
                    .table(SourceRepoEntry::Table)
                    .col(SourceRepoEntry::RepoId)
                    .col(SourceRepoEntry::Name)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(SourceRepoEntry::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum SourceRepoEntry {
    Table,
    RepoId,
    ExternalId,
    Name,
    Version,
    IconUrl,
    DownloadUrl,
    Languages,
    ContentRating,
    BaseUrl,
    MinAppVersion,
    FetchedAt,
}

/// Declared again rather than imported: a migration is a snapshot of the
/// schema at a point in time, and sharing identifiers across migrations means
/// renaming a column later silently rewrites history.
#[derive(DeriveIden)]
enum SourceRepo {
    Table,
    Id,
}
