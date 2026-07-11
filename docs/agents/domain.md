# Domain docs

This is a single-context repository. These rules describe how engineering skills consume its domain documentation while exploring the codebase.

## Before exploring, read these

- `CONTEXT.md` at the repository root.
- Relevant architectural decision records under `docs/adr/`.

If either location does not exist, proceed silently. Do not flag its absence or suggest creating it up front. The domain-modeling workflows create domain documentation lazily when terms or decisions are resolved.

## File structure

```text
/
├── CONTEXT.md
├── docs/
│   └── adr/
└── src/
```

## Use the glossary’s vocabulary

When output names a domain concept—in an issue title, refactor proposal, hypothesis, or test name—use the term defined in `CONTEXT.md`. Do not drift to synonyms that the glossary explicitly avoids.

If a needed concept is absent, reconsider whether it belongs to the project’s language or note the gap for domain modeling.

## Flag ADR conflicts

If proposed work contradicts an existing ADR, surface that conflict explicitly instead of silently overriding the decision.
