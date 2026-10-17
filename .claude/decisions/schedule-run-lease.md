# `replicas = "one"` claims an occurrence, not the job

A `"one"` job fires each occurrence on the one replica whose claim succeeds; the
claim holds that occurrence and nothing else, so a run outlasting its period
overlaps the next on another replica.

A run lease holding the job across replicas was built during 7.0 and removed by
owner decision. Every way it failed — a renewal unanswered once, a reply lost
after the claim took it, a deploy landing in a claim's window — held the job on
every replica, the claimer included, for the lease's length, and each lost
occurrence was put down at `debug` to a run that did not exist. A job whose runs
must not overlap keeps them shorter than its period, or guards its work where
the work is.
