---
issue: Closes #1439
decision: the skill classifier points at a skill on its own once `models.classifier` names a checkpoint: an unset `classifier.threshold` is 0.9, not shadow mode (revises D297's record-only default) | the owner: "once laya plugin is enabled, it should default be on. it should not default to watch only"; on the owner's Mac 14 answered decisions in two weeks reached confidence up to 0.9998 and none fired, because no threshold was set | read an unset threshold as no threshold again in `classifier::threshold` and make `Sidecar::threshold` an `Option<f64>`
raise: crate runtime +14, tests +128
---
The skill classifier is on as soon as it is set up. Before, naming a checkpoint in `models.classifier` only made it record its guesses: nothing pointed at a skill until `classifier.threshold` was set by hand, and `yi setup` never set one, so every new install ran it watch-only. Now an unset threshold is 0.9, so a guess at or above that confidence puts `Relevant: skill://NAME (the classifier, 0.93)` in front of the model, and a lower threshold in the config still wins. Command approval is unchanged; it was already on by default. The settings page says the unset value is 0.9. A test pins the 0.9 default and a set threshold overriding it; it was seen red with the default mutated to 1.1, and the record-only test now runs below a 0.95 threshold.

An answer is judged at the confidence the sidecar reported, and a `threshold: 1` or
higher never points: the firing condition skips it, so threshold 1 stays the record-only
setting; the settings row says so.
