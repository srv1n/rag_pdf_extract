# Agent Feedback

- context: Canonical vault ownership with separate Git worktree checks
- friction: Source provenance records the canonical checkout revision rather than the code revision tested in the invoking worktree.
- product-idea: Resolve the source checkout independently of the vault path and record the checked worktree commit.
- impact: Evidence requires manual qualification before reviewers can identify the tested code.
- related: TSK-T-0005 review commit244c6460aa3b4b42029d30d3c749ce40955cd397 recorded canonical02af59cab9
