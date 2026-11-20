---
issue: Closes #2302
raise: crate types +10
cut: none crates/permission/src/review.rs
cut: none crates/permission/tests/review.rs
cut: none crates/runtime/src/auto_review.rs
cut: none crates/runtime/src/gate.rs
cut: none crates/runtime/src/packed.rs
cut: none crates/runtime/src/permission.rs
cut: none crates/runtime/src/permission/inflight.rs
cut: none crates/runtime/src/prompts/auto_review.md
cut: none crates/runtime/tests/auto_review.rs
cut: none crates/runtime/tests/classifier_e2e.rs
cut: none crates/runtime/tests/sandbox_seam.rs
cut: none crates/types/src/config.rs
cut: none docs/YI_DESIGN.md
decision: Model auto-review is deleted, and `ask_user` is registered in every session with no `request` argument (amends D81 and D389, and the §8 auto-review flow) | the owner chose it as the feature that leaves for `desktop/` (#2257) and asked for a code cut; the classifier (D301) already settles what it can, so an auto-mode ask it leaves open goes to the user, and with no reviewer there is no denial for `ask_user` to escalate | revert by restoring `crates/runtime/src/auto_review.rs`, `ActionLedger` in yi-permission and `models.autoReview` from the commit before this one
ledger-: model auto-review M7/M8
ledger~: model recommender | summarizer, autoReview, selector | summarizer, selector
ledger~: runtime facts on the tools: bash contract, chain-stop note, reducer ranges, `ask_user` where someone answers | `auto_review::a_root_nobody_attends_is_handed_the_default_and_goes_on` | `ask_user::a_root_nobody_attends_is_handed_the_default_and_goes_on`
---
Model auto-review is gone. An auto-mode ask the classifier does not settle now goes straight to the user, as it did whenever `models.autoReview` was unset. `ask_user` is registered in every session as before, but its `request` argument, which put an auto-review denial to the user, is removed. A config that still names `models.autoReview` loads, ignores the key and prints one notice naming it; the settings page, `yi models recommend --role autoReview` and the `review.timeout_s` lever are gone. Sessions that journaled a `reviewer` answer still load and replay. A call the user refused is still remembered, so a later confident classifier answer cannot overrule it.
