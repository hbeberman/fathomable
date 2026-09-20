// @okf-doc: /decisions/0093-version-scoped-viewer-membership.md
//! Version-scoped membership for normal viewer surfaces.

use fathomable_core::annotations::{OriginSide, OriginVersion, Thread, ThreadId};
use fathomable_core::workspace::{ComparisonEndpoint, HeadObservation};

use crate::app::{App, Popup};

/// Exact context whose content is installed on normal viewer surfaces.
#[derive(Debug, Clone)]
pub(crate) struct Presentation {
    source: Option<ComparisonEndpoint>,
    target: Option<ComparisonEndpoint>,
    #[allow(dead_code, reason = "retained as a stale-observation identity guard")]
    head: HeadObservation,
    #[allow(dead_code, reason = "retained as the accepted point identity")]
    source_point: Option<String>,
    #[allow(dead_code, reason = "retained as the accepted mutable-index identity")]
    index_generation: Option<u64>,
}

impl App {
    pub(crate) fn accepted_presentation(&self) -> Option<Presentation> {
        let head = self.comparison.accepted_head()?.clone();
        let (source, target) = self.comparison.accepted_endpoints(self.diff_mode);
        let source_point = match source {
            Some(ComparisonEndpoint::ReviewPoint(id)) => Some(id.clone()),
            _ => None,
        };
        Some(Presentation {
            source: source.cloned(),
            target: target.cloned(),
            head,
            source_point,
            index_generation: self
                .comparison
                .accepted_index()
                .map(|(generation, _)| generation),
        })
    }

    /// Whether a thread belongs to normal, comparison-scoped surfaces.
    pub(crate) fn normal_thread(&self, thread: &Thread) -> bool {
        if thread.is_archived() {
            return false;
        }
        if self.draft_parent_visible(thread.id()) {
            return true;
        }
        self.normal_thread_without_draft(thread)
    }

    /// Normal membership without the narrow active/parked draft exception.
    pub(crate) fn normal_thread_without_draft(&self, thread: &Thread) -> bool {
        !thread.is_archived()
            && self
                .thread_matches_presentation(thread)
                .unwrap_or_else(|| self.reach.includes(thread))
    }

    /// A definitive endpoint match, or `None` for legacy mutable origins.
    pub(crate) fn thread_matches_presentation(&self, thread: &Thread) -> Option<bool> {
        let presentation = self.accepted_presentation();
        let source = presentation
            .as_ref()
            .and_then(|presentation| presentation.source.as_ref());
        let target = presentation
            .as_ref()
            .and_then(|presentation| presentation.target.as_ref());
        let accepted_head = presentation
            .as_ref()
            .and_then(|presentation| presentation.head.state().commit())
            .map(fathomable_core::workspace::CommitId::as_str);
        let source_commit = endpoint_commit(source, accepted_head);
        let target_commit = endpoint_commit(target, accepted_head);
        let commit_visible =
            |commit: &str| source_commit == Some(commit) || target_commit == Some(commit);

        match thread.origin_version() {
            OriginVersion::Commit { id } => Some(
                target_commit == Some(id.as_str())
                    || (thread.origin_side() == OriginSide::Base
                        && source_commit == Some(id.as_str())),
            ),
            OriginVersion::ReviewPoint { id, .. } => Some(
                matches!(source, Some(ComparisonEndpoint::ReviewPoint(point)) if point == id)
                    || thread.landed_commit().is_some_and(commit_visible),
            ),
            OriginVersion::WorkingTree { .. } | OriginVersion::Index { .. } => {
                thread.landed_commit().map(commit_visible)
            }
            OriginVersion::EmptyTree | OriginVersion::Unknown => None,
        }
    }

    fn draft_parent_visible(&self, id: &ThreadId) -> bool {
        let is_parent = |compose: &super::Compose| compose.target().thread() == Some(id);
        matches!(&self.popup, Some(Popup::Compose(compose)) if is_parent(compose))
            || self
                .docs
                .iter()
                .filter_map(|doc| doc.draft.as_ref())
                .any(is_parent)
    }

    /// Drop a normal-surface cursor once its thread leaves the installed
    /// presentation. Explicit history views keep repository-wide cursors.
    pub(crate) fn reconcile_normal_thread_cursor(&mut self) {
        if self.review_list.is_open() && self.review.view != super::list::ReviewView::Board {
            return;
        }
        let hidden = self
            .thread_cursor()
            .thread()
            .and_then(|id| self.thread(id))
            .is_some_and(|thread| !self.normal_thread(thread));
        if hidden {
            self.clear_thread_cursor();
        }
    }
}

fn endpoint_commit<'a>(
    endpoint: Option<&'a ComparisonEndpoint>,
    accepted_head: Option<&'a str>,
) -> Option<&'a str> {
    match endpoint {
        Some(ComparisonEndpoint::Commit(id)) => Some(id.as_str()),
        Some(ComparisonEndpoint::WorkingTree | ComparisonEndpoint::Index) => accepted_head,
        _ => None,
    }
}
