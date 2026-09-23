//! Port implementations over [`Repositories`].
//!
//! The repository struct keeps inherent methods with names that say what they
//! touch — `upsert_manga`, `list_chapters`, `due_follows` — because that is
//! what reads well at a call site inside this crate. The ports use short names
//! scoped by their trait. Rather than pick one and make the other awkward,
//! this module is the one-line-per-method bridge between them.
//!
//! One `Repositories` value implements every repository port, so the
//! composition root clones it into each `Arc<dyn …>` instead of holding a
//! separate object per port.

use nyuka_domain::Result;
use nyuka_domain::model::*;
use nyuka_domain::ports::{
    ChapterRepository, FollowRepository, MangaRepository, SourceRepository, UserRepository,
};

use crate::repository::Repositories;

#[async_trait::async_trait]
impl MangaRepository for Repositories {
    async fn get(&self, id: MangaId) -> Result<Manga> {
        self.get_manga(id).await
    }

    async fn find_by_external(&self, source: SourceId, key: &ExternalKey) -> Result<Option<Manga>> {
        self.find_manga_by_external(source, key).await
    }

    async fn upsert(&self, manga: &Manga) -> Result<MangaId> {
        self.upsert_manga(manga).await
    }

    async fn list(&self, cursor: Option<&Cursor>) -> Result<Page<Manga>> {
        self.list_manga(cursor).await
    }

    async fn list_summaries(&self, cursor: Option<&Cursor>) -> Result<Page<MangaSummary>> {
        self.list_manga_summaries(cursor).await
    }
}

#[async_trait::async_trait]
impl ChapterRepository for Repositories {
    async fn get(&self, id: ChapterId) -> Result<Chapter> {
        self.get_chapter(id).await
    }

    async fn list_for_manga(
        &self,
        manga: MangaId,
        cursor: Option<&Cursor>,
    ) -> Result<Page<Chapter>> {
        self.list_chapters(manga, cursor).await
    }

    async fn upsert_many(
        &self,
        manga: MangaId,
        chapters: &[SourceChapter],
    ) -> Result<Vec<ChapterId>> {
        self.upsert_chapters(manga, chapters).await
    }

    async fn downloaded(&self, id: ChapterId) -> Result<Option<DownloadedChapter>> {
        Repositories::downloaded(self, id).await
    }

    async fn record_download(&self, download: &DownloadedChapter) -> Result<()> {
        Repositories::record_download(self, download).await
    }

    async fn forget_downloads(&self, missing: &[ChapterId]) -> Result<u64> {
        Repositories::forget_downloads(self, missing).await
    }
}

#[async_trait::async_trait]
impl FollowRepository for Repositories {
    async fn get(&self, id: FollowId) -> Result<Follow> {
        self.get_follow(id).await
    }

    async fn list(&self, cursor: Option<&Cursor>) -> Result<Page<Follow>> {
        self.list_follows(cursor).await
    }

    async fn due(&self, limit: u64) -> Result<Vec<Follow>> {
        self.due_follows(limit).await
    }

    async fn upsert(&self, follow: &Follow) -> Result<FollowId> {
        self.upsert_follow(follow).await
    }

    async fn delete(&self, id: FollowId) -> Result<()> {
        self.delete_follow(id).await
    }

    async fn mark_checked(&self, id: FollowId) -> Result<()> {
        self.mark_follow_checked(id).await
    }
}

#[async_trait::async_trait]
impl SourceRepository for Repositories {
    async fn get(&self, id: SourceId) -> Result<InstalledSource> {
        self.get_source(id).await
    }

    async fn list(&self) -> Result<Vec<InstalledSource>> {
        self.list_sources().await
    }

    async fn upsert(&self, source: &InstalledSource) -> Result<SourceId> {
        self.upsert_source(source).await
    }

    async fn remove(&self, id: SourceId) -> Result<()> {
        self.delete_source(id).await
    }

    async fn list_repos(&self) -> Result<Vec<SourceRepo>> {
        self.list_source_repos().await
    }

    async fn list_entries(&self, repo: SourceRepoId) -> Result<Vec<SourceEntry>> {
        self.list_repo_entries(repo).await
    }

    async fn get_entry(&self, repo: SourceRepoId, external: &ExternalKey) -> Result<SourceEntry> {
        self.get_repo_entry(repo, external).await
    }

    async fn replace_entries(&self, repo: SourceRepoId, entries: &[SourceEntry]) -> Result<u64> {
        self.replace_repo_entries(repo, entries).await
    }

    async fn get_repo(&self, id: SourceRepoId) -> Result<SourceRepo> {
        self.get_source_repo(id).await
    }

    async fn upsert_repo(&self, repo: &SourceRepo) -> Result<SourceRepoId> {
        self.upsert_source_repo(repo).await
    }

    async fn remove_repo(&self, id: SourceRepoId) -> Result<()> {
        self.delete_source_repo(id).await
    }

    async fn mark_repo_refreshed(&self, id: SourceRepoId) -> Result<()> {
        Repositories::mark_repo_refreshed(self, id).await
    }

    async fn kv_get(&self, source: SourceId, key: &str) -> Result<Option<Vec<u8>>> {
        Repositories::kv_get(self, source, key).await
    }

    async fn kv_set(&self, source: SourceId, key: &str, value: Vec<u8>) -> Result<()> {
        Repositories::kv_set(self, source, key, value).await
    }

    async fn kv_list(&self, source: SourceId) -> Result<Vec<(String, Vec<u8>)>> {
        Repositories::kv_list(self, source).await
    }
}

#[async_trait::async_trait]
impl UserRepository for Repositories {
    async fn get(&self, id: UserId) -> Result<User> {
        self.get_user(id).await
    }

    async fn record_sign_in(&self, issuer: &str, subject: &str) -> Result<User> {
        Repositories::record_sign_in(self, issuer, subject).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// The composition root hands these around as trait objects, so they have
    /// to be object safe. A port that is not would fail here rather than in
    /// whichever crate first tried to store one.
    #[test]
    fn every_repository_port_is_object_safe() {
        fn assert_ports(repos: Repositories) {
            let shared = Arc::new(repos);
            let _: Arc<dyn MangaRepository> = shared.clone();
            let _: Arc<dyn ChapterRepository> = shared.clone();
            let _: Arc<dyn FollowRepository> = shared.clone();
            let _: Arc<dyn SourceRepository> = shared.clone();
            let _: Arc<dyn UserRepository> = shared;
        }
        // Never called: this is a compile-time assertion about the types.
        let _ = assert_ports;
    }
}
