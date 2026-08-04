# `runtimes/`

The Brain multiplexer, the credential-free Hands guest agent, and the Hands
guest image definition.

`hands-image` is built and validated locally only: rootfs definition, package
manifest and boot tests. No image is pushed from this workspace.

Each of the eight release ZIPs carries
`microvm-image-registration.json` (`aex.microvm-image-registration.v1`). The
descriptor fixes the plane-neutral `CreateMicrovmImage` configuration: variant,
managed base-image ARN template, ARM64 CPU, memory floor, OS capabilities and
hook states/timeouts. It intentionally has no S3 URI, build role, image name,
logging destination, tags or client token; the private release lane supplies
those custody-bound values after it has verified and copied the immutable ZIP.
The resulting immutable provider identity is the pair `imageArn` plus
`imageVersion`, never a synthesized version ARN.
