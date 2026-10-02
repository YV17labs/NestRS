# `#[process(concurrency = N)]` bounds what its name says

1.2.0 removed the `concurrency` key because it capped nothing: it only sized
apalis's read buffer, and scale was left to replicas alone. 7.0 restored it with
a real meaning — at most `N` attempts of one method at once on one replica, from
a permit pool of its own — and replicas remain the horizontal bound, added by a
queue-depth autoscaler rather than CPU, which an I/O-bound worker leaves flat.
The reason for the removal went with the defect. It is not a backend capability:
every backend bounds in-process parallelism.
