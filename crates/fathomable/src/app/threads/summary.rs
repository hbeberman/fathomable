// @okf-doc: /decisions/0086-one-thread-summary-and-its-actions.md
//! Shared thread summary facts and one-row header layout.

use std::ops::Range;

use fathomable_core::annotations::{AutoResolve, Lifecycle, Placement, Thread, ThreadId};
use fathomable_core::layout::{display_width, graphemes};

use crate::app::threads::author_label;

/// Facts shared by inline headers, review headers, and sidebar cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadSummary {
    id: ThreadId,
    lifecycle: Lifecycle,
    auto_resolve: AutoResolve,
    location: String,
    commit_reference: Option<[u8; 8]>,
    context: Option<String>,
    author: String,
    author_is_user: bool,
    preview: String,
    replies: usize,
    modified: u64,
}

impl ThreadSummary {
    pub(crate) fn new(
        thread: &Thread,
        placement: Option<Placement>,
        user: &str,
        context: Option<&str>,
    ) -> Self {
        let latest = thread.messages().next_back();
        let author = latest.map_or(thread.author(), |message| message.author());
        let body = latest.map_or(thread.comment(), |message| message.body());
        Self {
            id: thread.id().clone(),
            lifecycle: thread.lifecycle(),
            auto_resolve: thread.auto_resolve(),
            location: location(thread, placement),
            commit_reference: Self::short_commit_reference(thread.origin_version()),
            context: context.map(str::to_owned),
            author: author_label(author, user),
            author_is_user: author.is_user(),
            preview: body.lines().next().unwrap_or("").to_owned(),
            replies: thread.messages().len().saturating_sub(1),
            modified: thread.modified(),
        }
    }

    pub(crate) fn id(&self) -> &ThreadId {
        &self.id
    }

    #[cfg(test)]
    pub(crate) fn lifecycle(&self) -> Lifecycle {
        self.lifecycle
    }

    pub(crate) fn location(&self) -> &str {
        &self.location
    }

    pub(crate) fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    pub(crate) fn commit_reference(&self) -> Option<&str> {
        self.commit_reference
            .as_ref()
            .and_then(|reference| std::str::from_utf8(reference).ok())
    }

    pub(crate) fn author(&self) -> &str {
        &self.author
    }

    fn short_commit_reference(
        version: &fathomable_core::annotations::OriginVersion,
    ) -> Option<[u8; 8]> {
        let reference = version.commit_reference()?;
        let short = reference.as_bytes().get(..7)?;
        short.iter().all(u8::is_ascii_hexdigit).then(|| {
            let mut token = [0; 8];
            token[0] = b'@';
            token[1..].copy_from_slice(short);
            token
        })
    }

    pub(crate) fn author_is_user(&self) -> bool {
        self.author_is_user
    }

    pub(crate) fn preview(&self) -> &str {
        &self.preview
    }

    pub(crate) fn replies(&self) -> usize {
        self.replies
    }

    pub(crate) fn modified(&self) -> u64 {
        self.modified
    }

    pub(crate) fn glyph(&self) -> &'static str {
        match self.lifecycle {
            Lifecycle::Active => "●",
            Lifecycle::ResolutionProposed => "◐",
            Lifecycle::Resolved => "○",
        }
    }

    fn status(&self) -> Option<&'static str> {
        if self.auto_resolve.is_enabled() {
            Some("autoresolve")
        } else if self.lifecycle == Lifecycle::ResolutionProposed {
            Some("resolve proposed")
        } else {
            None
        }
    }
}

fn location(thread: &Thread, placement: Option<Placement>) -> String {
    match placement {
        Some(Placement::File) => "file".to_owned(),
        Some(Placement::Anchored(range) | Placement::Edited(range)) => format!("L{range}"),
        Some(Placement::Detached(range)) => format!("L{range}?"),
        None => thread
            .range()
            .map_or_else(|| "file".to_owned(), |range| format!("L{range}")),
    }
}

/// A semantic style owned by the summary layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SummaryTone {
    Surface,
    Info,
    Status,
    Lifecycle(Lifecycle),
    Author { user: bool },
    Preview,
    Chevron,
}

/// One styled run in a laid-out summary row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SummarySpan {
    pub(crate) text: String,
    pub(crate) tone: SummaryTone,
}

impl SummarySpan {
    fn new(text: impl Into<String>, tone: SummaryTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

/// A complete one-row factual summary and its disclosure geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SummaryLayout {
    pub(crate) spans: Vec<SummarySpan>,
    disclosure: Range<usize>,
}

impl SummaryLayout {
    pub(crate) fn disclosure_at(&self, column: usize) -> bool {
        self.disclosure.contains(&column)
    }
}

/// Inputs that vary between inline and review header surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SummaryLayoutOptions {
    pub(crate) width: usize,
    pub(crate) leading: usize,
    pub(crate) expanded: bool,
}

/// Lay out one inline or review header.
#[expect(
    clippy::too_many_lines,
    reason = "Keeping allocation and hit-region decisions in one pure pass prevents geometry drift."
)]
pub(crate) fn layout(
    summary: &ThreadSummary,
    options: SummaryLayoutOptions,
    now: u64,
) -> SummaryLayout {
    let mut spans = Vec::new();
    let mut used = options.leading;
    let reserve_commit = usize::from(summary.commit_reference().is_some()) * 8;
    let prefix_budget = options
        .width
        .saturating_sub(options.leading + reserve_commit);

    push_clipped(
        &mut spans,
        &mut used,
        options.width,
        summary.glyph(),
        SummaryTone::Lifecycle(summary.lifecycle),
    );
    let gap = usize::from(prefix_budget >= 3);
    if gap > 0 {
        push_clipped(
            &mut spans,
            &mut used,
            options.width,
            " ",
            SummaryTone::Surface,
        );
    }
    let disclosure_start = used;
    let chevron = if prefix_budget >= 6 {
        if options.expanded { "▾   " } else { "▸   " }
    } else if prefix_budget >= 4 {
        if options.expanded { "▾ " } else { "▸ " }
    } else {
        if options.expanded { "▾" } else { "▸" }
    };
    push_clipped(
        &mut spans,
        &mut used,
        options.width,
        chevron,
        SummaryTone::Chevron,
    );
    let disclosure_end = (disclosure_start + display_width(chevron)).min(options.width);

    let mut facts = Vec::with_capacity(6);
    if let Some(status) = summary.status() {
        facts.push((Fact::Status, status.to_owned()));
    }
    if let Some(context) = summary.context() {
        facts.push((Fact::Context, context.to_owned()));
    }
    facts.push((Fact::Location, summary.location().to_owned()));
    if let Some(reference) = summary.commit_reference() {
        facts.push((Fact::Commit, reference.to_owned()));
    }
    if summary.replies() > 0 {
        facts.push((Fact::Replies, format!("↩{}", summary.replies())));
    }
    facts.push((Fact::Modified, compact_age(summary.modified(), now)));

    let prefix_used = used;
    let mut shown = facts;
    while fixed_width(prefix_used, &shown) > options.width {
        let remove = [
            Fact::Context,
            Fact::Modified,
            Fact::Replies,
            Fact::Status,
            Fact::Location,
        ]
        .into_iter()
        .find(|fact| shown.iter().any(|(candidate, _)| candidate == fact));
        let Some(remove) = remove else {
            break;
        };
        shown.retain(|(fact, _)| *fact != remove);
    }

    let tail_width = facts_width(&shown);
    let separators = usize::from(tail_width > 0) * 2;
    let available_preview = options
        .width
        .saturating_sub(prefix_used + tail_width + separators);
    if !options.expanded {
        push_preview(
            summary,
            available_preview,
            &mut spans,
            &mut used,
            options.width,
        );
    }

    if tail_width > 0 && used < options.width {
        let start = options.width.saturating_sub(tail_width);
        if start > used {
            let padding = start - used;
            push_clipped(
                &mut spans,
                &mut used,
                options.width,
                &" ".repeat(padding),
                SummaryTone::Surface,
            );
        }
        for (index, (fact, text)) in shown.iter().enumerate() {
            if index > 0 {
                push_clipped(
                    &mut spans,
                    &mut used,
                    options.width,
                    "  ",
                    SummaryTone::Surface,
                );
            }
            push_clipped(
                &mut spans,
                &mut used,
                options.width,
                text,
                if *fact == Fact::Status {
                    SummaryTone::Status
                } else {
                    SummaryTone::Info
                },
            );
        }
    }
    if used < options.width {
        spans.push(SummarySpan::new(
            " ".repeat(options.width - used),
            SummaryTone::Surface,
        ));
    }
    SummaryLayout {
        spans,
        disclosure: disclosure_start..disclosure_end,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fact {
    Status,
    Context,
    Location,
    Commit,
    Replies,
    Modified,
}

fn facts_width(facts: &[(Fact, String)]) -> usize {
    facts
        .iter()
        .map(|(_, text)| display_width(text))
        .sum::<usize>()
        + 2 * facts.len().saturating_sub(1)
}

fn fixed_width(prefix: usize, facts: &[(Fact, String)]) -> usize {
    let tail = facts_width(facts);
    prefix + tail + usize::from(tail > 0) * 2
}

fn push_preview(
    summary: &ThreadSummary,
    width: usize,
    spans: &mut Vec<SummarySpan>,
    used: &mut usize,
    total: usize,
) {
    let author_width = display_width(summary.author());
    if width <= author_width + 1 {
        return;
    }
    push_clipped(
        spans,
        used,
        total,
        summary.author(),
        SummaryTone::Author {
            user: summary.author_is_user(),
        },
    );
    push_clipped(spans, used, total, " ", SummaryTone::Surface);
    let preview = ellipsize(summary.preview(), width - author_width - 1);
    push_clipped(spans, used, total, &preview, SummaryTone::Preview);
}

fn push_clipped(
    spans: &mut Vec<SummarySpan>,
    used: &mut usize,
    width: usize,
    text: &str,
    tone: SummaryTone,
) {
    let available = width.saturating_sub(*used);
    if available == 0 {
        return;
    }
    let clipped = clip(text, available);
    let cells = display_width(&clipped);
    if cells == 0 {
        return;
    }
    spans.push(SummarySpan::new(clipped, tone));
    *used += cells;
}

fn clip(text: &str, width: usize) -> String {
    let mut used = 0;
    let mut end = 0;
    for (offset, grapheme) in graphemes(text) {
        let cells = display_width(grapheme);
        if cells > width.saturating_sub(used) {
            break;
        }
        used += cells;
        end = offset + grapheme.len();
    }
    text[..end].to_owned()
}

fn ellipsize(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = clip(text, width - 1);
    out.push('…');
    out
}

/// Compact thread modification age.
pub(crate) fn compact_age(modified: u64, now: u64) -> String {
    let elapsed = now.saturating_sub(modified);
    match elapsed {
        0..60 => "now".to_owned(),
        60..3600 => format!("{}m", elapsed / 60),
        3600..86_400 => format!("{}h", elapsed / 3600),
        _ => format!("{}d", elapsed / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use fathomable_core::annotations::{
        Author, Draft, LineRange, MessageTarget, OriginVersion, Reply, Store, UserSubmit,
    };
    use fathomable_testing::TempDir;

    use super::*;

    fn text(layout: &SummaryLayout) -> String {
        layout
            .spans
            .iter()
            .map(|span| span.text.as_str())
            .collect::<String>()
    }

    fn fixture(name: &str, author: Author) -> anyhow::Result<(TempDir, Store, ThreadId)> {
        let dir = TempDir::new(name)?;
        let mut store = Store::open(dir.0.join("threads.jsonl"))?;
        let id = store.annotate(
            Draft::new(author, Path::new("a.md"), LineRange::new(42, 46), "opening"),
            &"\n".repeat(50),
            1,
        )?;
        Ok((dir, store, id))
    }

    #[test]
    fn facts_use_append_order_and_thread_modification() -> anyhow::Result<()> {
        let (_dir, mut store, id) = fixture("summary-facts", Author::agent("starter"))?;
        store.reply(&id, Reply::new(Author::User, 2, "latest line\nsecond"))?;
        store.edit_user(
            &id,
            MessageTarget::Comment,
            "edited opening",
            300,
            UserSubmit::Normal,
        )?;
        let summary = ThreadSummary::new(
            store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?,
            Some(Placement::Anchored(LineRange::new(42, 46))),
            "User",
            Some("feature"),
        );
        assert_eq!(summary.author(), "User");
        assert_eq!(summary.preview(), "latest line");
        assert_eq!(summary.replies(), 1);
        assert_eq!(summary.modified(), 300);
        assert_eq!(summary.context(), Some("feature"));
        Ok(())
    }

    #[test]
    fn collapsed_layout_has_preview_tail_and_detached_suffix() -> anyhow::Result<()> {
        let (_dir, store, id) = fixture("summary-detached", Author::agent("starter"))?;
        let summary = ThreadSummary::new(
            store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?,
            Some(Placement::Detached(LineRange::new(42, 46))),
            "User",
            None,
        );
        let row = layout(
            &summary,
            SummaryLayoutOptions {
                width: 60,
                leading: 0,
                expanded: false,
            },
            301,
        );
        let text = text(&row);
        assert!(text.starts_with("● ▸   starter opening"), "{text}");
        assert!(text.contains("L42-46?"), "{text}");
        assert!(text.trim_end().ends_with("5m"), "{text}");
        assert!(!text.contains('↩'));
        Ok(())
    }

    #[test]
    fn headers_show_passive_lifecycle_status() -> anyhow::Result<()> {
        let (_dir, mut store, id) = fixture("summary-lifecycle-status", Author::User)?;
        store.reply_user(&id, 2, "reply", UserSubmit::EnableAutoResolve)?;
        let summary = ThreadSummary::new(
            store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?,
            Some(Placement::Anchored(LineRange::new(42, 46))),
            "User",
            None,
        );
        let row = layout(
            &summary,
            SummaryLayoutOptions {
                width: 80,
                leading: 1,
                expanded: true,
            },
            2,
        );
        let rendered = text(&row);
        assert!(rendered.contains("autoresolve"), "{rendered}");
        assert!(
            row.spans
                .iter()
                .any(|span| { span.text == "autoresolve" && span.tone == SummaryTone::Status })
        );
        assert!(!rendered.contains("Auto-resolve"), "{rendered}");
        assert!(!rendered.contains("Resolve"), "{rendered}");
        assert!(!rendered.contains("User reply"), "{rendered}");
        assert!(row.disclosure_at(3));
        assert!(row.disclosure_at(6));
        assert!(!row.disclosure_at(7));

        let (_dir, mut proposed_store, proposed_id) =
            fixture("summary-proposed-status", Author::User)?;
        proposed_store.reply(
            &proposed_id,
            Reply::new(Author::agent("agent"), 2, "done").proposing_resolution(),
        )?;
        let proposed = ThreadSummary::new(
            proposed_store
                .thread(&proposed_id)
                .ok_or_else(|| anyhow::anyhow!("thread"))?,
            Some(Placement::Anchored(LineRange::new(42, 46))),
            "User",
            None,
        );
        let proposed = layout(
            &proposed,
            SummaryLayoutOptions {
                width: 80,
                leading: 1,
                expanded: true,
            },
            2,
        );
        assert!(text(&proposed).contains("resolve proposed"));
        assert!(
            proposed.spans.iter().any(|span| {
                span.text == "resolve proposed" && span.tone == SummaryTone::Status
            })
        );
        Ok(())
    }

    #[test]
    fn resolved_headers_are_factual_at_wide_and_narrow_widths() -> anyhow::Result<()> {
        let (_dir, mut store, id) = fixture("summary-actions", Author::User)?;
        store.resolve(&id, None, 3)?;
        let summary = ThreadSummary::new(
            store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?,
            Some(Placement::Anchored(LineRange::new(42, 46))),
            "User",
            None,
        );
        for (width, expanded) in [(50, true), (18, false)] {
            let rendered = text(&layout(
                &summary,
                SummaryLayoutOptions {
                    width,
                    leading: 0,
                    expanded,
                },
                3,
            ));
            assert!(!rendered.contains("Archive"), "{rendered}");
            assert!(!rendered.contains("Restore"), "{rendered}");
            assert!(!rendered.contains(" a"), "{rendered}");
            assert!(!rendered.contains(" u"), "{rendered}");
        }

        Ok(())
    }

    #[test]
    fn archived_headers_are_factual() -> anyhow::Result<()> {
        let (_dir, mut store, id) = fixture("summary-archived-actions", Author::User)?;
        store.resolve(&id, None, 3)?;
        store.archive(&id, 4)?;
        let summary = ThreadSummary::new(
            store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?,
            Some(Placement::Detached(LineRange::new(42, 46))),
            "User",
            None,
        );
        let row = layout(
            &summary,
            SummaryLayoutOptions {
                width: 50,
                leading: 0,
                expanded: true,
            },
            4,
        );
        let rendered = text(&row);
        assert!(!rendered.contains("Restore"), "{rendered}");
        assert!(!rendered.contains("Reopen"), "{rendered}");
        assert!(!rendered.contains("Archive"), "{rendered}");
        Ok(())
    }

    #[test]
    fn narrow_layout_drops_facts_in_order_and_respects_unicode_widths() -> anyhow::Result<()> {
        let (_dir, mut store, id) = fixture("summary-narrow", Author::agent("机器人"))?;
        store.reply(&id, Reply::new(Author::agent("机器人"), 2, "界面 preview"))?;
        let summary = ThreadSummary::new(
            store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?,
            Some(Placement::Anchored(LineRange::new(42, 46))),
            "User",
            Some("feature"),
        );
        let wide = layout(
            &summary,
            SummaryLayoutOptions {
                width: 50,
                leading: 0,
                expanded: false,
            },
            122,
        );
        assert_eq!(display_width(&text(&wide)), 50);
        assert!(text(&wide).contains("feature"));
        assert!(text(&wide).contains("↩1"));
        let narrow = layout(
            &summary,
            SummaryLayoutOptions {
                width: 14,
                leading: 0,
                expanded: false,
            },
            122,
        );
        let text = text(&narrow);
        assert_eq!(display_width(&text), 14);
        assert!(!text.contains("feature"));
        assert!(!text.contains("↩1"));
        assert!(!text.contains("2m"));
        assert!(text.contains("L42-46"));
        Ok(())
    }

    #[test]
    fn full_headers_show_every_captured_commit_reference_and_keep_it_narrow() -> anyhow::Result<()>
    {
        for version in [
            OriginVersion::commit("c403a01abcdef"),
            OriginVersion::working_tree(Some("c403a01abcdef".to_owned())),
            OriginVersion::index(Some("c403a01abcdef".to_owned())),
            OriginVersion::review_point("point", Some("c403a01abcdef".to_owned())),
        ] {
            let token = ThreadSummary::short_commit_reference(&version);
            assert_eq!(
                token
                    .as_ref()
                    .and_then(|reference| std::str::from_utf8(reference).ok()),
                Some("@c403a01")
            );
        }
        let versions = [OriginVersion::commit("c403a01abcdef")];
        for (index, version) in versions.into_iter().enumerate() {
            let dir = TempDir::new(&format!("summary-origin-{index}"))?;
            let mut store = Store::open(dir.0.join("threads.jsonl"))?;
            let id = store.annotate(
                Draft::new(
                    Author::User,
                    Path::new("a.md"),
                    LineRange::new(42, 46),
                    "opening",
                )
                .at_source(version, fathomable_core::annotations::OriginSide::Target),
                &"\n".repeat(50),
                1,
            )?;
            let summary = ThreadSummary::new(
                store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?,
                Some(Placement::Anchored(LineRange::new(42, 46))),
                "User",
                Some("lower priority"),
            );
            assert_eq!(summary.commit_reference(), Some("@c403a01"));
            let wide = text(&layout(
                &summary,
                SummaryLayoutOptions {
                    width: 80,
                    leading: 1,
                    expanded: true,
                },
                120,
            ));
            assert!(
                wide.find("L42-46") < wide.find("@c403a01"),
                "location precedes reference: {wide}"
            );
            let narrow = text(&layout(
                &summary,
                SummaryLayoutOptions {
                    width: 20,
                    leading: 3,
                    expanded: true,
                },
                120,
            ));
            assert!(narrow.contains("@c403a01"), "{narrow}");
            assert!(!narrow.contains("lower priority"), "{narrow}");
            assert!(!narrow.contains("2m"), "{narrow}");
        }
        Ok(())
    }

    #[test]
    fn minimum_inline_header_keeps_the_full_commit_and_disclosure_hit_region() -> anyhow::Result<()>
    {
        let dir = TempDir::new("summary-minimum-inline-origin")?;
        let mut store = Store::open(dir.0.join("threads.jsonl"))?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "opening",
            )
            .at_source(
                OriginVersion::commit("abcdef0123456789"),
                fathomable_core::annotations::OriginSide::Target,
            ),
            "line\n",
            1,
        )?;
        let summary = ThreadSummary::new(
            store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?,
            Some(Placement::Anchored(LineRange::new(1, 1))),
            "User",
            None,
        );
        let row = layout(
            &summary,
            SummaryLayoutOptions {
                width: 14,
                leading: 1,
                expanded: false,
            },
            1,
        );
        let rendered = text(&row);
        assert!(rendered.contains("@abcdef0"), "{rendered}");
        let chevron_byte = rendered
            .find('▸')
            .ok_or_else(|| anyhow::anyhow!("chevron"))?;
        let chevron = 1 + display_width(&rendered[..chevron_byte]);
        assert!(row.disclosure_at(chevron));
        let commit_byte = rendered
            .find("@abcdef0")
            .ok_or_else(|| anyhow::anyhow!("commit"))?;
        let commit = 1 + display_width(&rendered[..commit_byte]);
        assert!((commit..commit + 8).all(|column| !row.disclosure_at(column)));
        Ok(())
    }

    #[test]
    fn origins_without_a_captured_commit_omit_the_reference() -> anyhow::Result<()> {
        for version in [
            OriginVersion::working_tree(None),
            OriginVersion::index(None),
            OriginVersion::review_point("point", None),
            OriginVersion::EmptyTree,
            OriginVersion::Unknown,
        ] {
            assert_eq!(ThreadSummary::short_commit_reference(&version), None);
        }
        for (index, version) in [OriginVersion::Unknown].into_iter().enumerate() {
            let dir = TempDir::new(&format!("summary-no-origin-{index}"))?;
            let mut store = Store::open(dir.0.join("threads.jsonl"))?;
            let id = store.annotate(
                Draft::new(
                    Author::User,
                    Path::new("a.md"),
                    LineRange::new(1, 1),
                    "opening",
                )
                .at_source(version, fathomable_core::annotations::OriginSide::Target),
                "line\n",
                1,
            )?;
            let summary = ThreadSummary::new(
                store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?,
                Some(Placement::Anchored(LineRange::new(1, 1))),
                "User",
                None,
            );
            assert_eq!(summary.commit_reference(), None);
        }
        Ok(())
    }
}
