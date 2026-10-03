# Local Patches

This vendored copy is based on `matroska-demuxer` 0.8.1 from crates.io.

Margins carries one local compatibility patch in `src/ebml.rs`: EBML data-size
VINTs whose value bits are all one are decoded as unknown size (`u64::MAX`) for
every encoded length from 1 through 8 bytes. Chrome MediaRecorder emits WebM
files with 8-byte unknown-sized Segment and Cluster elements
(`01 ff ff ff ff ff ff ff`), and treating that value as an ordinary known size
causes the demuxer to seek far beyond EOF before cluster parsing.

The local tests cover unknown-size encodings and maximum-minus-one known sizes
for all supported EBML data-size VINT lengths.
