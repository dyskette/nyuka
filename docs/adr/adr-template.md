# ADR-NNNN: <Decision stated as an action>

| | |
|---|---|
| **Status** | Proposed \| Accepted \| Superseded \| Deprecated |
| **Date** | YYYY-MM-DD |
| **Deciders** | <email> |
| **Applies to** | <crate, directory, or subsystem> |
| **Supersedes** | None \| ADR-NNNN |

<One or two sentences stating what this article explains: the choice made, and what it costs to maintain. Write it as a lead paragraph, not as a summary of the sections below.>

## Context

<The forces that constrain the decision. Prefer constraints that were already fixed over general-purpose comparisons: a framework comparison is weak evidence, an existing requirement that eliminates a candidate is strong evidence.>

<Use a subheading per constraint when there is more than one. State each constraint in terms of what the system must do, not what the team prefers.>

## Decision

<The choice, in present tense, as a statement about the system: "The API uses X", not "We decided to use X". Follow with a short list of the specifics that make the choice actionable: versions pinned, patterns adopted, boundaries enforced.>

> [!IMPORTANT]
> <Use a callout only for something a future maintainer can get wrong. Delete this block if there is nothing to warn about.>

## Consequences

### What you gain

<Concrete outcomes, not restated benefits. Each item should name the thing that now works and why.>

### What it costs you

<Every accepted cost, including the ones that argue against the decision. If a cost is a scheduled future task, say what triggers it and what it touches. Use a table when there are several discrete items.>

### Follow-up work this decision creates

<Numbered list of tasks the decision produces: pins to add, tests to write, CI checks to enforce the boundary. Keep these actionable enough to become issues.>

## Alternatives considered

| Alternative | Version or status | Outcome |
|---|---|---|
| <name> | <version> | Rejected. <One-line reason.> |

### <Strongest alternative>

<Give the alternative its best case first, including the criteria on which it wins. Then state the reasons for rejection in order of weight. An ADR that makes the rejected option look weak is not useful six months later.>

## Revisit this decision when

<Specific, observable triggers. "When requirements change" is not a trigger. "When the API runs more than one instance" is.>

## References

- [<Primary source: changelog, issue, specification, or documentation>](url)

<!--
Style rules for ADRs in this repository:

- Write in the voice of product documentation: second person, active voice, present
  tense, short declarative sentences. Address a maintainer reading this in a year.
- Say "the API uses X" or "you get Y", not "we chose X" or "I think Y".
- No marketing adjectives. "Axum is built on hyper 1.x" earns its place;
  "Axum is a blazing-fast modern framework" does not.
- Tables for comparisons. Prose for reasoning. Never prose for a comparison that
  has more than two dimensions.
- Callouts are [!NOTE], [!IMPORTANT], and [!WARNING], used sparingly. Two per
  document is usually too many.
- Format identifiers, crate names, environment variables, and file paths as code.
- Link to primary sources: changelogs, issues, specifications, and official docs.
  Cite blog posts only for directional claims, and label them as directional.
- File name: NNNN-decision-as-an-action.md, four-digit sequence, no gaps.
-->
