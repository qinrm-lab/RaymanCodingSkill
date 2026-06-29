# Extracted Skill Rules

Source: `SKILL.md`

## Documentation Structure And Lossless Splitting

- Keep explanatory docs layered and easy to index: start from `docs/README.md`, group related topics, and maintain task-oriented links for readers who know what they need.
- Preserve existing document paths when possible; add or update index pages before moving files so existing references do not break.
- Keep the main `SKILL.md` focused on core rules with a 100-line target and a 125-line整理 trigger, so one extra line does not force churn.
- When a skill rule Markdown file grows beyond 20,000 characters, or the main `SKILL.md` grows past the 125-line trigger, run `rayman docs compact-skill-rules`; it must losslessly split exact rule text into linked `references/` files until the source file is below 12,000 characters, the main `SKILL.md` is back within its 100-line target, and the source file is at least 20% smaller.
- Skill rule splitting is lossless: move detailed procedures, long examples, background, and edge cases into linked reference files; never delete, summarize, or paraphrase rules to fit the budget.
