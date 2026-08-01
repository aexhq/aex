# `workers/`

Deployable event, queue, stream and schedule binaries. Each directory is one
independently publishable artifact with one composition root and one companion
package under `tests/live/`.

A worker owns exactly one authority and links no code capable of writing a
sibling authority.
