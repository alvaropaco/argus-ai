# ARGUS Specifications

Feature specifications live under numbered directories:

```text
specs/
├── 001-bootstrap/
│   ├── spec.md
│   ├── plan.md
│   └── tasks.md
└── NNN-feature-name/
    ├── spec.md
    ├── plan.md
    ├── tasks.md
    └── contracts/
```

## Workflow

1. Define the problem and acceptance criteria in `spec.md`.
2. Produce the implementation strategy in `plan.md`.
3. Break implementation into testable tasks in `tasks.md`.
4. Implement the tasks.
5. Verify against the Constitution and applicable ADRs.
6. Update architecture documentation when implementation changes the realized architecture.
7. Create or update an ADR when a durable architectural decision changes.

## Relationship to architecture documentation

- `.specify/memory/constitution.md` — non-negotiable engineering principles.
- `docs/adr/` — durable architectural decisions.
- `docs/architecture/` — current system architecture and domain model.
- `specs/` — feature-level requirements and implementation planning.
- `crates/` — implementation.
