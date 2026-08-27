# Electives are root-level Groups with their own Offerings

The original model enrolled a student into a Seminar belonging to a *different*
Cohort. That put them in the other Cohort's subtree, which made them an attendee
of its entire cohort-wide lecture series. One shared student then made two
Cohorts' lectures mutually exclusive, and with 30% cross-enrolment across 80
cohorts **94.8% of lecture pairs conflicted** — 1,146 Sessions needing at most
350 slots. Construction left 2,468 of 25,520 unplaced and no solver could have
done better.

Electives are now root-level Groups with their own Offerings, which is what an
elective actually is. They stay tree-unrelated to the student's home Seminar, so
`PersonDoubleBooking` still has work the Group check cannot do, without welding
two Cohorts together.

Elective groups are **Class-sized**. Seminar-sized produced 360 groups at
large-university scale whose Offerings added 56% to total demand — an elective
programme larger than the core curriculum.
