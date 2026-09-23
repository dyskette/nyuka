//! The `update_sources` handler.
//!
//! Walks every configured repository and refreshes its index, so the catalog
//! a user browses reflects what the repository publishes today rather than
//! what it published when the repository was added.
//!
//! # One failing repository does not stop the others
//!
//! A repository is a URL an operator chose, and one of them being unreachable
//! is ordinary. Aborting the pass on the first failure would mean a single
//! dead repository silently freezes every other one's index. Each is
//! refreshed independently, and the job fails at the end only if *every*
//! repository failed — which is the shape that means the network or the
//! process is wrong, not one URL.
//!
//! # The stamp is the record of success
//!
//! `last_refreshed_at` is written per repository, only on success. A
//! repository that has been failing for a week therefore reads as a week
//! stale, which is the thing an operator needs to see.

use std::sync::Arc;

use nyuka_domain::model::Job;
use nyuka_domain::ports::{SourceRegistry, SourceRepository};
use nyuka_domain::{DomainError, Result};

use super::KindHandler;

/// What one pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UpdateReport {
    pub refreshed: usize,
    pub failed: usize,
}

pub struct UpdateSources {
    sources: Arc<dyn SourceRepository>,
    registry: Arc<dyn SourceRegistry>,
}

impl UpdateSources {
    pub fn new(sources: Arc<dyn SourceRepository>, registry: Arc<dyn SourceRegistry>) -> Self {
        Self { sources, registry }
    }

    pub async fn run(&self) -> Result<UpdateReport> {
        let repos = self.sources.list_repos().await?;
        if repos.is_empty() {
            // Not a failure. A fresh install has no repositories until an
            // operator adds one, and the scheduled job runs from day one.
            return Ok(UpdateReport::default());
        }

        let mut report = UpdateReport::default();
        for repo in &repos {
            match self.registry.refresh_repo(repo.id).await {
                Ok(()) => {
                    self.sources.mark_repo_refreshed(repo.id).await?;
                    report.refreshed += 1;
                }
                Err(e) => {
                    report.failed += 1;
                    tracing::warn!(
                        repo.id = %repo.id,
                        repo.url = %repo.url,
                        error = %e,
                        "refreshing a source repository failed; continuing with the rest"
                    );
                }
            }
        }
        Ok(report)
    }
}

#[async_trait::async_trait]
impl KindHandler for UpdateSources {
    async fn handle(&self, _job: &Job) -> Result<()> {
        let report = self.run().await?;

        if report.refreshed == 0 && report.failed > 0 {
            // Every repository failed, so this is not one bad URL. Retryable:
            // the usual cause is the network being down, which it will not be
            // forever.
            return Err(DomainError::Source {
                message: format!(
                    "all {} source repositories failed to refresh",
                    report.failed
                ),
                retryable: true,
            });
        }

        tracing::info!(
            refreshed = report.refreshed,
            failed = report.failed,
            "refreshed source repositories"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::testing::job;
    use nyuka_domain::model::{
        Capability, ExternalKey, InstalledSource, JobKind, SourceEntry, SourceId, SourceRepo,
        SourceRepoId,
    };
    use std::collections::HashSet;
    use std::sync::Mutex;
    use uuid::Uuid;

    struct Shared<T>(Arc<T>);

    impl<T> Clone for Shared<T> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }

    impl<T> std::ops::Deref for Shared<T> {
        type Target = T;
        fn deref(&self) -> &T {
            &self.0
        }
    }

    fn repo(name: &str) -> SourceRepo {
        SourceRepo {
            id: SourceRepoId(Uuid::new_v4()),
            name: name.into(),
            url: format!("https://example.invalid/{name}.json"),
            last_refreshed_at: None,
            created_at: chrono::Utc::now(),
        }
    }

    struct Sources {
        repos: Vec<SourceRepo>,
        stamped: Mutex<Vec<SourceRepoId>>,
    }

    #[async_trait::async_trait]
    impl SourceRepository for Shared<Sources> {
        async fn get(&self, _id: SourceId) -> Result<InstalledSource> {
            Err(DomainError::NotFound)
        }
        async fn list(&self) -> Result<Vec<InstalledSource>> {
            Ok(vec![])
        }
        async fn upsert(&self, source: &InstalledSource) -> Result<SourceId> {
            Ok(source.id)
        }
        async fn remove(&self, _id: SourceId) -> Result<()> {
            Ok(())
        }
        async fn list_repos(&self) -> Result<Vec<SourceRepo>> {
            Ok(self.repos.clone())
        }
        async fn list_entries(&self, _repo: SourceRepoId) -> Result<Vec<SourceEntry>> {
            Ok(vec![])
        }
        async fn get_entry(
            &self,
            _repo: SourceRepoId,
            _external: &ExternalKey,
        ) -> Result<SourceEntry> {
            Err(DomainError::NotFound)
        }
        async fn replace_entries(
            &self,
            _repo: SourceRepoId,
            entries: &[SourceEntry],
        ) -> Result<u64> {
            Ok(entries.len() as u64)
        }
        async fn get_repo(&self, _id: SourceRepoId) -> Result<SourceRepo> {
            Err(DomainError::NotFound)
        }
        async fn upsert_repo(&self, repo: &SourceRepo) -> Result<SourceRepoId> {
            Ok(repo.id)
        }
        async fn remove_repo(&self, _id: SourceRepoId) -> Result<()> {
            Ok(())
        }
        async fn mark_repo_refreshed(&self, id: SourceRepoId) -> Result<()> {
            self.stamped.lock().expect("lock").push(id);
            Ok(())
        }
        async fn kv_get(&self, _s: SourceId, _k: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        async fn kv_set(&self, _s: SourceId, _k: &str, _v: Vec<u8>) -> Result<()> {
            Ok(())
        }
        async fn kv_list(&self, _s: SourceId) -> Result<Vec<(String, Vec<u8>)>> {
            Ok(vec![])
        }
    }

    /// Fails for the repositories named in `failing`, succeeds for the rest.
    struct Registry {
        failing: HashSet<SourceRepoId>,
        attempted: Mutex<Vec<SourceRepoId>>,
    }

    #[async_trait::async_trait]
    impl SourceRegistry for Shared<Registry> {
        async fn refresh_repo(&self, repo: SourceRepoId) -> Result<()> {
            self.attempted.lock().expect("lock").push(repo);
            if self.failing.contains(&repo) {
                return Err(DomainError::Source {
                    message: "index is unreachable".into(),
                    retryable: true,
                });
            }
            Ok(())
        }
        async fn install(
            &self,
            _repo: SourceRepoId,
            _entry: &ExternalKey,
        ) -> Result<InstalledSource> {
            Err(DomainError::NotFound)
        }
        async fn uninstall(&self, _source: SourceId) -> Result<()> {
            Ok(())
        }
        fn supported_capabilities(&self) -> &[Capability] {
            &[]
        }
        async fn settings_declaration(&self, _s: SourceId) -> Result<serde_json::Value> {
            Ok(serde_json::Value::Array(vec![]))
        }
    }

    struct Setup {
        handler: UpdateSources,
        sources: Shared<Sources>,
        registry: Shared<Registry>,
        repos: Vec<SourceRepo>,
    }

    fn setup(count: usize, failing: &[usize]) -> Setup {
        let repos: Vec<SourceRepo> = (0..count).map(|i| repo(&format!("r{i}"))).collect();
        let failing: HashSet<SourceRepoId> = failing.iter().map(|i| repos[*i].id).collect();
        let sources = Shared(Arc::new(Sources {
            repos: repos.clone(),
            stamped: Mutex::new(vec![]),
        }));
        let registry = Shared(Arc::new(Registry {
            failing,
            attempted: Mutex::new(vec![]),
        }));
        Setup {
            handler: UpdateSources::new(Arc::new(sources.clone()), Arc::new(registry.clone())),
            sources,
            registry,
            repos,
        }
    }

    fn update_job() -> Job {
        job(JobKind::UpdateSources, serde_json::json!({}))
    }

    #[tokio::test]
    async fn every_repository_is_refreshed_and_stamped() {
        let s = setup(3, &[]);
        s.handler.handle(&update_job()).await.expect("refreshed");

        assert_eq!(s.registry.attempted.lock().expect("lock").len(), 3);
        assert_eq!(s.sources.stamped.lock().expect("lock").len(), 3);
    }

    /// One dead URL must not freeze every other repository's index.
    #[tokio::test]
    async fn a_failing_repository_does_not_stop_the_others() {
        let s = setup(3, &[1]);
        s.handler
            .handle(&update_job())
            .await
            .expect("two of three worked, so the job worked");

        assert_eq!(
            s.registry.attempted.lock().expect("lock").len(),
            3,
            "the pass must continue past the failure, not abort on it"
        );
        let stamped = s.sources.stamped.lock().expect("lock").clone();
        assert_eq!(stamped.len(), 2);
        assert!(
            !stamped.contains(&s.repos[1].id),
            "a repository that failed must keep reading as stale"
        );
    }

    /// All of them failing is a different shape: the network or the process is
    /// wrong, not one URL.
    #[tokio::test]
    async fn every_repository_failing_fails_the_job_retryably() {
        let s = setup(2, &[0, 1]);
        let err = s
            .handler
            .handle(&update_job())
            .await
            .expect_err("nothing refreshed");

        assert!(err.to_string().contains("all 2 source repositories"));
        assert!(
            err.is_retryable(),
            "the usual cause is the network, which will not be down forever"
        );
        assert!(s.sources.stamped.lock().expect("lock").is_empty());
    }

    /// A fresh install has no repositories, and the scheduled job runs from
    /// day one.
    #[tokio::test]
    async fn no_repositories_is_not_a_failure() {
        let s = setup(0, &[]);
        s.handler
            .handle(&update_job())
            .await
            .expect("nothing to do is not an error");
    }
}
