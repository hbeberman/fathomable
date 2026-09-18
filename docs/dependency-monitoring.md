---
type: Software
title: Dependency monitoring
description: Automated version updates, RustSec checks, and the GitHub settings maintained outside the repository.
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
