// @okf-doc: /decisions/0093-version-scoped-viewer-membership.md
//! Version-scoped membership for normal viewer surfaces.

use fathomable_core::annotations::{OriginSide, OriginVersion, Thread, ThreadId};
use fathomable_core::workspace::{CheckoutIdentity, ComparisonEndpoint, HeadObservation};

use crate::app::{App, Popup};

/// Exact context whose content is installed on normal viewer surfaces.
#[derive(Debug, Clone)]
pub(crate) struct Presentation {
    source: Option<ComparisonEndpoint>,
    target: Option<ComparisonEndpoint>,
    head: HeadObservation,
    checkout: CheckoutIdentity,
}

impl App {
    pub(crate) fn accepted_presentation(&self) -> Option<Presentation> {
        let head = self.comparison.accepted_head()?.clone();
        let (source, target) = self.comparison.accepted_endpoints(self.diff_mode);
        Some(Presentation {
            source: source.cloned(),
            target: target.cloned(),
            head,
            checkout: self.workspace.identity(),
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
        !thread.is_archived() && self.thread_matches_presentation(thread)
    }

    /// Whether immutable provenance belongs to the accepted presentation.
    pub(crate) fn thread_matches_presentation(&self, thread: &Thread) -> bool {
        let Some(presentation) = self.accepted_presentation() else {
            return false;
        };
        match thread.origin_side() {
            OriginSide::Base => {
                let Some(source) = presentation.source.as_ref() else {
                    return false;
                };
                if let Some(comparison) = thread.comparison() {
                    let Some(target) = presentation.target.as_ref() else {
                        return false;
                    };
                    (!comparison.is_checkout_scoped()
                        || comparison.checkout() == Some(&presentation.checkout))
                        && self.endpoint_matches_version(
                            source,
                            comparison.base(),
                            &presentation.head,
                        )
                        && self.endpoint_matches_version(
                            target,
                            comparison.target(),
                            &presentation.head,
                        )
                        && self.endpoint_matches_origin(source, thread)
                } else {
                    self.endpoint_matches_origin(source, thread)
                }
            }
            OriginSide::Target | OriginSide::Unspecified => presentation
                .target
                .as_ref()
                .is_some_and(|target| self.endpoint_matches_origin(target, thread)),
        }
    }

    /// Whether the thread may project into the currently rendered content.
    pub(crate) fn inline_thread(&self, thread: &Thread) -> bool {
        !thread.is_archived() && self.thread_matches_presentation(thread)
    }

    fn endpoint_matches_origin(&self, endpoint: &ComparisonEndpoint, thread: &Thread) -> bool {
        match (endpoint, thread.origin_version()) {
            (ComparisonEndpoint::Commit(actual), OriginVersion::Commit { id }) => {
                actual.as_str() == id
            }
            (ComparisonEndpoint::WorkingTree, OriginVersion::WorkingTree { .. }) => thread
                .provenance()
                .working_tree()
                .is_some_and(|facts| facts.checkout() == &self.workspace.identity()),
            (ComparisonEndpoint::Index, OriginVersion::Index { .. }) => thread
                .provenance()
                .index()
                .is_some_and(|facts| facts.checkout() == &self.workspace.identity()),
            (ComparisonEndpoint::ReviewPoint(actual), OriginVersion::ReviewPoint { id, base }) => {
                actual == id
                    && thread.provenance().review_point().is_some_and(|facts| {
                        self.review_points
                            .as_ref()
                            .and_then(|store| store.get(actual))
                            .is_some_and(|point| {
                                facts.checkout() == &point.checkout_identity()
                                    && facts.base() == base.as_deref()
                                    && point.head().map(ToString::to_string).as_deref()
                                        == base.as_deref()
                            })
                    })
            }
            (ComparisonEndpoint::EmptyTree, OriginVersion::EmptyTree) => true,
            _ => false,
        }
    }

    fn endpoint_matches_version(
        &self,
        endpoint: &ComparisonEndpoint,
        expected: &OriginVersion,
        head: &HeadObservation,
    ) -> bool {
        match (endpoint, expected) {
            (ComparisonEndpoint::Commit(actual), OriginVersion::Commit { id }) => {
                actual.as_str() == id
            }
            (ComparisonEndpoint::WorkingTree, OriginVersion::WorkingTree { observed_head })
            | (ComparisonEndpoint::Index, OriginVersion::Index { observed_head }) => {
                head.state()
                    .commit()
                    .map(fathomable_core::workspace::CommitId::as_str)
                    == observed_head.as_deref()
            }
            (ComparisonEndpoint::ReviewPoint(actual), OriginVersion::ReviewPoint { id, base }) => {
                actual == id
                    && self
                        .review_points
                        .as_ref()
                        .and_then(|store| store.get(actual))
                        .is_some_and(|point| {
                            point.head().map(ToString::to_string).as_ref() == base.as_ref()
                        })
            }
            (ComparisonEndpoint::EmptyTree, OriginVersion::EmptyTree) => true,
            _ => false,
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
