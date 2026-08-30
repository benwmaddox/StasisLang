# Task 388 IT-018 visual evidence

The Android x86_64 release-shell seam ran on `emulator-5554` from source
commit `8d47a5862eb25c5b8516fdb3ea72a56dfd6a2dd9`. The run passed all five
ordered touch probes, the final state checksum (`3215`), the stable command
trace (`2627465271`), the final command trace (`3512156903`), and the named
pixel-region checks.

Visual evidence: [final PNG](../../artifacts/task-388-it018-visual/stable-frame.png)
was inspected and shows the real Android compositor with black pillarbox bars,
the green released state centered in the logical viewport, and the magenta
pointer marker at the completed drag location. The inspected
[drag MP4](../../artifacts/task-388-it018-visual/drag-sequence.mp4) shows the
unchanged red baseline through the outside-letterbox gesture, followed by the
inside drag's red-to-yellow/blue-to-green state progression and the magenta
marker moving from the down location to the released lower-right location.
