// @okf-doc: /decisions/0086-one-thread-summary-and-its-actions.md
//! Shared thread summary facts and one-row header layout.

use std::ops::Range;

use fathomable_core::annotations::{AutoResolve, Lifecycle, Placement, Thread, ThreadId};
use fathomable_core::layout::{display_width, graphemes};

use crate::app::input::bindings::Action;
use crate::app::threads::author_label;

/// Facts shared by inline headers, review headers, and sidebar cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ThreadSummary {
    id: ThreadId,
    lifecycle: Lifecycle,
    auto_resolve: AutoResolve,
    archived: bool,
    location: String,
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
            archived: thread.is_archived(),
            location: location(thread, placement),
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

    pub(crate) fn author(&self) -> &str {
        &self.author
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

    pub(crate) fn actions(&self) -> Vec<Action> {
        if self.archived {
            return vec![Action::RestoreThread];
        }
        match self.lifecycle {
            Lifecycle::Active | Lifecycle::ResolutionProposed => {
                vec![Action::ToggleAutoResolve, Action::ToggleResolved]
            }
            Lifecycle::Resolved => vec![Action::ToggleResolved, Action::ArchiveThread],
        }
    }

    fn action_label(&self, action: Action) -> &'static str {
        match action {
            Action::ToggleAutoResolve if self.auto_resolve.is_enabled() => "Disable auto-resolve",
            Action::ToggleAutoResolve => "Auto-resolve",
            Action::ToggleResolved if self.lifecycle == Lifecycle::Resolved => "Reopen",
            Action::ToggleResolved => "Resolve",
            Action::ArchiveThread => "Archive",
            Action::RestoreThread => "Restore",
            _ => "",
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
    Lifecycle(Lifecycle),
    Author { user: bool },
    Preview,
    Chevron,
    Action,
    ActionKey,
}

/// One styled run in a laid-out summary row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SummarySpan {
    pub(crate) text: String,
    pub(crate) tone: SummaryTone,
    pub(crate) action: Option<Action>,
}

impl SummarySpan {
    fn new(text: impl Into<String>, tone: SummaryTone) -> Self {
        Self {
            text: text.into(),
            tone,
            action: None,
        }
    }

    fn action(text: impl Into<String>, tone: SummaryTone, action: Action) -> Self {
        Self {
            text: text.into(),
            tone,
            action: Some(action),
        }
    }
}

/// Exact cells occupied by a summary control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SummaryRegion {
    pub(crate) cells: Range<usize>,
    pub(crate) action: Action,
}

/// A complete one-row summary and its visible hit geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SummaryLayout {
    pub(crate) spans: Vec<SummarySpan>,
    disclosure: Range<usize>,
    actions: Vec<SummaryRegion>,
}

impl SummaryLayout {
    pub(crate) fn action_at(&self, column: usize) -> Option<Action> {
        self.actions
            .iter()
            .find(|region| region.cells.contains(&column))
            .map(|region| region.action)
    }

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
    pub(crate) cursor: bool,
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
    let mut actions = Vec::new();
    let mut used = options.leading;
    let disclosure_start = used + 2;

    push_clipped(
        &mut spans,
        &mut used,
        options.width,
        summary.glyph(),
        SummaryTone::Lifecycle(summary.lifecycle),
        None,
        &mut actions,
    );
    push_clipped(
        &mut spans,
        &mut used,
        options.width,
        " ",
        SummaryTone::Surface,
        None,
        &mut actions,
    );
    push_clipped(
        &mut spans,
        &mut used,
        options.width,
        if options.expanded { "▾   " } else { "▸   " },
        SummaryTone::Chevron,
        None,
        &mut actions,
    );
    let disclosure_end = (disclosure_start + 4).min(options.width);

    let action_specs = (options.expanded || options.cursor).then(|| summary.actions());
    let action_width = action_specs.as_ref().map_or(0, |items| {
        items
            .iter()
            .enumerate()
            .map(|(index, action)| {
                usize::from(index > 0) * 2
                    + display_width(summary.action_label(*action))
                    + if options.cursor { 3 } else { 0 }
            })
            .sum()
    });

    let mut facts = Vec::with_capacity(4);
    if let Some(context) = summary.context() {
        facts.push((Fact::Context, context.to_owned()));
    }
    facts.push((Fact::Location, summary.location().to_owned()));
    if summary.replies() > 0 {
        facts.push((Fact::Replies, format!("↩{}", summary.replies())));
    }
    facts.push((Fact::Modified, compact_age(summary.modified(), now)));

    let prefix_used = used;
    let mut shown = facts;
    while fixed_width(prefix_used, action_width, &shown) > options.width {
        let remove = [Fact::Context, Fact::Replies, Fact::Modified, Fact::Location]
            .into_iter()
            .find(|fact| shown.iter().any(|(candidate, _)| candidate == fact));
        let Some(remove) = remove else {
            break;
        };
        shown.retain(|(fact, _)| *fact != remove);
    }

    let tail_width = facts_width(&shown);
    let separators = usize::from(tail_width > 0) * 2;
    let preview_action_gap = usize::from(!options.expanded && action_width > 0) * 2;
    let available_preview = options
        .width
        .saturating_sub(prefix_used + action_width + tail_width + separators + preview_action_gap);
    if !options.expanded {
        let before = used;
        push_preview(
            summary,
            available_preview,
            &mut spans,
            &mut used,
            options.width,
            &mut actions,
        );
        if used > before && action_width > 0 {
            push_clipped(
                &mut spans,
                &mut used,
                options.width,
                "  ",
                SummaryTone::Surface,
                None,
                &mut actions,
            );
        }
    }

    if let Some(action_specs) = action_specs {
        for (index, action) in action_specs.into_iter().enumerate() {
            if index > 0 {
                push_clipped(
                    &mut spans,
                    &mut used,
                    options.width,
                    "  ",
                    SummaryTone::Surface,
                    None,
                    &mut actions,
                );
            }
            push_clipped(
                &mut spans,
                &mut used,
                options.width,
                summary.action_label(action),
                SummaryTone::Action,
                Some(action),
                &mut actions,
            );
            if options.cursor {
                push_clipped(
                    &mut spans,
                    &mut used,
                    options.width,
                    match action {
                        Action::ToggleAutoResolve => "  R",
                        Action::ToggleResolved => "  r",
                        Action::ArchiveThread => "  a",
                        Action::RestoreThread => "  u",
                        _ => "",
                    },
                    SummaryTone::ActionKey,
                    Some(action),
                    &mut actions,
                );
            }
        }
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
                None,
                &mut actions,
            );
        }
        for (index, (_, text)) in shown.iter().enumerate() {
            if index > 0 {
                push_clipped(
                    &mut spans,
                    &mut used,
                    options.width,
                    "  ",
                    SummaryTone::Surface,
                    None,
                    &mut actions,
                );
            }
            push_clipped(
                &mut spans,
                &mut used,
                options.width,
                text,
                SummaryTone::Info,
                None,
                &mut actions,
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
        actions,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fact {
    Context,
    Location,
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

fn fixed_width(prefix: usize, actions: usize, facts: &[(Fact, String)]) -> usize {
    let tail = facts_width(facts);
    prefix + actions + tail + usize::from(tail > 0) * 2
}

fn push_preview(
    summary: &ThreadSummary,
    width: usize,
    spans: &mut Vec<SummarySpan>,
    used: &mut usize,
    total: usize,
    actions: &mut Vec<SummaryRegion>,
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
        None,
        actions,
    );
    push_clipped(spans, used, total, " ", SummaryTone::Surface, None, actions);
    let preview = ellipsize(summary.preview(), width - author_width - 1);
    push_clipped(
        spans,
        used,
        total,
        &preview,
        SummaryTone::Preview,
        None,
        actions,
    );
}

fn push_clipped(
    spans: &mut Vec<SummarySpan>,
    used: &mut usize,
    width: usize,
    text: &str,
    tone: SummaryTone,
    action: Option<Action>,
    regions: &mut Vec<SummaryRegion>,
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
    let start = *used;
    spans.push(match action {
        Some(action) => SummarySpan::action(clipped, tone, action),
        None => SummarySpan::new(clipped, tone),
    });
    *used += cells;
    if let Some(action) = action {
        if let Some(last) = regions
            .last_mut()
            .filter(|region| region.action == action && region.cells.end == start)
        {
            last.cells.end = *used;
        } else {
            regions.push(SummaryRegion {
                cells: start..*used,
                action,
            });
        }
    }
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
        Author, Draft, LineRange, MessageTarget, Reply, Store, UserSubmit,
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
                cursor: false,
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
    fn expanded_actions_put_words_before_subdued_keys() -> anyhow::Result<()> {
        let (_dir, mut store, id) = fixture("summary-actions", Author::User)?;
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
                cursor: true,
            },
            2,
        );
        let rendered = text(&row);
        assert!(
            rendered.contains("Disable auto-resolve  R  Resolve  r"),
            "{rendered}"
        );
        assert!(!rendered.contains("User reply"), "{rendered}");
        let resolve = row
            .actions
            .iter()
            .find(|region| region.action == Action::ToggleResolved)
            .ok_or_else(|| anyhow::anyhow!("resolve action"))?;
        assert_eq!(
            row.action_at(resolve.cells.start.saturating_sub(1)),
            None,
            "the separator is inert"
        );
        assert!(row.disclosure_at(3));
        assert!(row.disclosure_at(6));
        assert!(!row.disclosure_at(7));

        let non_cursor = layout(
            &summary,
            SummaryLayoutOptions {
                width: 80,
                leading: 1,
                expanded: true,
                cursor: false,
            },
            2,
        );
        let non_cursor_text = text(&non_cursor);
        assert!(
            non_cursor_text.contains("Disable auto-resolve  Resolve"),
            "{non_cursor_text}"
        );
        assert!(
            non_cursor
                .spans
                .iter()
                .all(|span| span.tone != SummaryTone::ActionKey)
        );

        let clipped = layout(
            &summary,
            SummaryLayoutOptions {
                width: 18,
                leading: 1,
                expanded: true,
                cursor: true,
            },
            2,
        );
        assert!(
            (0..18).all(|column| { clipped.action_at(column) != Some(Action::ToggleResolved) }),
            "a fully hidden action has no hit region"
        );

        store.resolve(&id, None, 3)?;
        let resolved = ThreadSummary::new(
            store.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?,
            Some(Placement::Anchored(LineRange::new(42, 46))),
            "User",
            None,
        );
        let resolved = layout(
            &resolved,
            SummaryLayoutOptions {
                width: 50,
                leading: 0,
                expanded: true,
                cursor: true,
            },
            3,
        );
        let rendered = text(&resolved);
        assert!(rendered.contains("Reopen  r"), "{rendered}");
        assert!(rendered.contains("Archive  a"), "{rendered}");
        assert!(!rendered.contains("Auto-resolve"), "{rendered}");

        Ok(())
    }

    #[test]
    fn archived_headers_offer_restore_only() -> anyhow::Result<()> {
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
                cursor: true,
            },
            4,
        );
        let rendered = text(&row);
        assert!(rendered.contains("Restore  u"), "{rendered}");
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
                cursor: false,
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
                cursor: false,
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
}
