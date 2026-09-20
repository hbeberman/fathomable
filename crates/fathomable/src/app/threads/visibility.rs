// @okf-doc: /decisions/0093-version-scoped-viewer-membership.md
//! Version-scoped membership for normal viewer surfaces.

use fathomable_core::annotations::{
    OriginSide, OriginVersion, ReviewAssociation, ReviewEndpoint, ReviewScope, Thread, ThreadId,
};
use fathomable_core::workspace::{ComparisonEndpoint, HeadObservation};

use crate::app::comparison::review_endpoint;
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
            && (self.thread_matches_focus(thread)
                || self
                    .thread_matches_presentation(thread)
                    .unwrap_or_else(|| self.reach.includes(thread)))
    }

    /// Whether a thread belongs to the exact native scope of this presentation.
    pub(crate) fn thread_matches_presentation(&self, thread: &Thread) -> Option<bool> {
        if matches!(
            thread.review_association(),
            ReviewAssociation::Content {
                endpoint: ReviewEndpoint::Unknown
            }
        ) {
            return None;
        }
        let Some(presentation) = self.accepted_presentation() else {
            return Some(false);
        };
        let source = presentation
            .source
            .as_ref()
            .map(|endpoint| review_endpoint(endpoint, &self.workspace));
        let target = presentation
            .target
            .as_ref()
            .map(|endpoint| review_endpoint(endpoint, &self.workspace));
        match thread.review_association() {
            ReviewAssociation::Review {
                scope: ReviewScope::Commit { .. } | ReviewScope::Mutable { .. },
            } => Some(false),
            ReviewAssociation::Review {
                scope:
                    ReviewScope::Comparison {
                        source: expected_source,
                        target: expected_target,
                    },
            } => Some(
                source.as_ref() == Some(expected_source)
                    && target.as_ref() == Some(expected_target),
            ),
            ReviewAssociation::Content { endpoint } => {
                let displayed = if thread.origin_side() == OriginSide::Base {
                    source.as_ref()
                } else {
                    target.as_ref()
                };
                let landed_off = thread.origin_side() != OriginSide::Base
                    && self.diff_mode() == fathomable_core::config::DiffMode::Off
                    && thread.landed_commit().is_some_and(|landed| {
                        matches!(
                            target.as_ref(),
                            Some(ReviewEndpoint::Commit { id }) if id == landed
                        )
                    });
                Some(displayed == Some(endpoint) || landed_off)
            }
        }
    }

    /// Whether a retained review explicitly owns this thread.
    pub(crate) fn thread_matches_focus(&self, thread: &Thread) -> bool {
        let Some(focus) = self.comparison.review_focus() else {
            return false;
        };
        thread.review_association().scope() == Some(focus)
            || matches!(
                focus,
                ReviewScope::Commit { target }
                    if thread.landed_commit() == Some(target.as_str())
                        && thread_origin_checkout(thread)
                            == Some(&self.workspace.identity())
            )
    }

    /// Whether the thread may project into the currently rendered content.
    pub(crate) fn inline_thread(&self, thread: &Thread) -> bool {
        if thread.is_archived() {
            return false;
        }
        if self.diff_mode() != fathomable_core::config::DiffMode::Off {
            return self.normal_thread(thread);
        }
        let Some(presentation) = self.accepted_presentation() else {
            return false;
        };
        let Some(target) = presentation.target.as_ref() else {
            return false;
        };
        if thread.origin_side() == OriginSide::Base {
            return false;
        }
        let target = review_endpoint(target, &self.workspace);
        thread_origin_endpoint(thread).is_some_and(|origin| origin == target)
            || thread.landed_commit().is_some_and(
                |landed| matches!(&target, ReviewEndpoint::Commit { id } if id == landed),
            )
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

fn thread_origin_endpoint(thread: &Thread) -> Option<ReviewEndpoint> {
    match thread.origin_version() {
        OriginVersion::Commit { id } => Some(ReviewEndpoint::commit(id)),
        OriginVersion::WorkingTree { .. } => thread
            .provenance()
            .working_tree()
            .map(|facts| ReviewEndpoint::working_tree(facts.checkout().clone())),
        OriginVersion::Index { .. } => thread
            .provenance()
            .index()
            .map(|facts| ReviewEndpoint::index(facts.checkout().clone())),
        OriginVersion::ReviewPoint { id, .. } => Some(ReviewEndpoint::review_point(id)),
        OriginVersion::EmptyTree => Some(ReviewEndpoint::EmptyTree),
        OriginVersion::Unknown => Some(ReviewEndpoint::Unknown),
    }
}

fn thread_origin_checkout(
    thread: &Thread,
) -> Option<&fathomable_core::workspace::CheckoutIdentity> {
    thread
        .provenance()
        .working_tree()
        .map(fathomable_core::annotations::WorkingTreeFacts::checkout)
        .or_else(|| {
            thread
                .provenance()
                .index()
                .map(fathomable_core::annotations::IndexFacts::checkout)
        })
        .or_else(|| {
            thread
                .provenance()
                .review_point()
                .map(fathomable_core::annotations::ReviewPointFacts::checkout)
        })
}
