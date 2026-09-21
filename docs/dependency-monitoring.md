---
type: Software
title: Dependency monitoring
description: Automated dependency checks and GitHub security settings maintained outside the repository.
resource: .github/dependabot.yml
related_resources:
  - .github/workflows/audit.yml
tags:
  - dependencies
---

# Dependency monitoring

Dependabot opens weekly version-update pull requests for Cargo, GitHub
Actions, and the Python documentation requirements. Routine minor and patch
updates are grouped per ecosystem, while major updates remain individual.
The conservative open-pull-request limits apply to version updates, not
security updates.

Repository configuration cannot enable every GitHub security feature.
Maintainers must separately enable the dependency graph, Dependabot alerts,
and Dependabot security updates under the repository's security settings.
Each maintainer or organization must also configure its own GitHub
notification and email preferences. Security updates are raised when
eligible alerts appear; they are not delayed until the weekly version-update
schedule.

RustSec maintains the Rust ecosystem's security advisory database;
`cargo audit` compares the locked crate versions against those advisories.
Advisories can be published after a dependency was merged.
The standalone `RustSec audit` workflow therefore runs `cargo audit` daily,
on relevant dependency changes, and on manual request. Each ephemeral job
fetches the current advisory database and checks `Cargo.lock` without
compiling the product. The existing local commit gate remains the immediate
pre-merge check. Enable Actions failure notifications as well as Dependabot
notifications to receive failures from this workflow. Scheduled workflows
run from the default branch; GitHub can disable them in public repositories
after 60 days without repository activity, so check that the schedule stays
enabled.

[Offline secret scanning](secret-scanning.md) adds pinned Betterleaks source
and artifact checks, PR/push introduced-commit scanning, and a weekly
full-fetched-history job. It complements, rather than enables or replaces,
GitHub native secret scanning and push protection. The binary version pin
requires maintainer review; Dependabot does not update it.

## Public-alpha security sign-off

The tracked [security policy](../SECURITY.md) directs reports to GitHub's
private vulnerability reporting channel. Before making the repository public
or publishing the first alpha, a maintainer completes the external settings
that repository contents cannot prove:

- [x] Add the root policy and link it from contributor and agent guidance.
- [ ] Enable private vulnerability reporting under **Settings > Security**.
- [ ] While signed in as a maintainer, verify access to
  `https://github.com/hbeberman/fathomable/security/advisories/new`.
- [ ] Verify every intended maintainer can access security advisories and has
  the desired repository security-alert notifications enabled.
- [ ] Verify native GitHub secret scanning and push protection are enabled
  where available, with appropriate maintainer notifications.
- [ ] Require the independent secret-scanning CI check and maintainer review
  of scanner/policy/workflow changes; verify its behavior on fork PRs.
- [ ] Fetch the intended branches and tags and pass `just secrets-history`
  locally immediately before publicity.
- [ ] Pass scans of the exact final source packages and all distributed
  artifacts after any transformations; keep publishing/tag/release creation manual.

Unchecked items remain release blockers; a policy link alone does not establish
that the private channel or notifications work.
