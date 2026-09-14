# Mission

Decision: 2026-09-13. This document sets the product direction. The [roadmap](roadmap.md)
defines the work and its acceptance criteria. The discovery and recipe system described here
is partly implemented: the repository supplies browser execution, local macros and a
[persistent discovery protocol](discovery.md). Independent recipe acceptance and the catalogue
remain planned.

## Make websites learnable by agents

chrome-agent's mission is to turn websites into capabilities that agents discover, verify,
reuse and maintain themselves.

A person provides an objective, access and the scope of permitted actions. The system explores
the relevant parts of a site, determines which paths achieve the objective, and preserves what
the evidence supports. Discovering a procedure must not require a person to write selectors,
demonstrate the steps or correct each attempt.

Discovery is driven by the calling agent. It supplies the reasoning and chooses experiments;
chrome-agent supplies the execution protocol, persistent knowledge and validation lifecycle.
The product does not host its own model or require a separate model-provider configuration.

Each useful exploration should make later tasks on that environment more directly executable.
An agent starting a fresh conversation should be able to use the resulting knowledge without
access to the conversation that produced it.

The product serves agents working through sites whose interfaces change and whose available
APIs do not cover the task. People delegate the result and control access; agents do the work
of understanding the site. Manual recipes remain possible, but autonomous discovery is the
behavior this project must demonstrate.

## The experience to build

An agent receives a request on an unfamiliar site, such as collecting the latest articles or
preparing a particular product variant in a cart. It finds applicable knowledge, checks its
conditions, and explores the missing parts. The next task reuses what still applies and can
combine capabilities into a different path.

The system preserves more than a successful action sequence: how to recognize the relevant
state, required inputs, which effects were observed, how to check the result, and known failure
cases. A successful trial establishes evidence for that context. Further trials establish
which inputs and situations the procedure supports.

A site change triggers targeted rediscovery and a new candidate revision. The required result
stays the same during repair. A run that cannot establish the result reports its uncertainty
and the evidence it retained.

Useful outcomes include connecting an unfamiliar application to an agent without authoring an
integration, combining previously discovered capabilities for a new request, and repairing a
changed route without rediscovering the entire site.

## Recipes, shared and private

A recipe packages a discovered capability for reuse. It carries its application conditions,
parameters, result checks, required permissions, executable procedure and validation evidence.
Publication status and the outcome of a current run are separate facts.

Git repositories provide version history and distribution. A project catalogue on GitHub is
part of the target product: agents can propose recipes and updates, and an independent
validation service can accept and merge eligible revisions under an explicit policy.
Maintainers govern that policy and its exceptions. Routine eligible changes should not depend
on a person approving every recipe.

The first public catalogue accepts read and extraction recipes. Recipes that modify a site
come later, once their effects and recovery can be independently tested in suitable environments.
This publication scope does not remove the existing local mutation commands.

The catalogue establishes a common acceptance process. Users can also keep local recipes,
operate private repositories, select trusted sources and pin revisions. Execution remains with
the user's browser and access. Private recipes support the same validation lifecycle and do
not need public publication to be useful.

Discovery records stay private by default. Public contribution requires a configured sharing
policy and a separately constructed package that excludes session data, credentials and private
records. Public and private recipe identities include their source; names alone cannot select
or replace an executable revision. Model access to page data is configured separately from
catalogue publication.

Trust must state what was checked, by which validator, for which revision and under which
conditions. Passing catalogue checks establishes eligibility under that policy. Every use
still checks the current context and requested result. The catalogue must support withdrawal,
and clients must make stale trust information visible.

## Product boundaries

Exploration follows useful objectives and an explicit budget. The site model is partial: an
unvisited route, unseen account type or untested operation remains unknown. Failed experiments
are knowledge too, provided their circumstances are retained.

Discovery must respect an execution policy. A page, recipe or model response cannot grant
permissions, widen the task, or change publication policy. Operations with external effects
need a permitted context for experimentation and a way to reconcile an uncertain result.

The current typed command validation is not an execution sandbox. Existing macros may include
arbitrary page JavaScript; example Python programs run with their process permissions. A public
recipe execution boundary must be built before claiming that automatically accepted recipes
are confined to their declared capabilities.

An exhaustive website crawler, a general workflow language and a hosted browser fleet are
outside the plan. A commercial marketplace, billing and vendor-specific memory integrations
are also outside the plan. The shared recipe catalogue is part of the product direction.

## What establishes progress

The decisive demonstration is an agent discovering a useful capability without a supplied
procedure, another agent reusing it with new inputs, and the system repairing a changed path
while preserving the result checks. All three must be evaluated against independently observed
outcomes.

Track correct completions, false successes, repeated discovery avoided, human procedural help,
repair scope, and exploration and validation cost. Include failures and uncertain runs in the
denominator. For shared recipes, also track successful use in a fresh environment, stale or
withdrawn revisions rejected, and private data excluded from contributions.

The first [isolated discovery experiment](experiments/discovery-2026-09-13.md) establishes
discovery, handoff and executable reuse on one synthetic site. Its independent tests also
reject the generated recipe for false completeness claims. Cross-site generalization and a
trustworthy recipe catalogue remain unproven. The execution foundation and its limits are
recorded in [task compilation](design/task-compilation.md).
