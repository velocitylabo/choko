# CLAUDE.md

## Git commit / PR rules

- コミットメッセージに `https://claude.ai/code/session_*` URL を含めないこと
- PR タイトル・本文にもセッション URL を含めないこと
- PRは origin に対して作成すること

## Commit conventions

Use conventional commits.

Format: `<type>(<scope>): <description>`

Types:
- `feat:` — new feature
- `fix:` — bug fix
- `docs:` — documentation only
- `ci:` — CI/workflow changes
- `chore:` — maintenance tasks
- `refactor:` — code restructuring without behavior change
- `test:` — adding or updating tests
