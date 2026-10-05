# Enzyme search-core subtree

> Cutover note (2026-10-02): this is historical import documentation. The
> vendored subtree was retired in favor of the private `enzyme-core` git
> dependency pinned at `d92f9e52ffddbe318ed2d6797cde2d821985c61a`.
> That commit's `crates/enzyme-core` tree is exactly the last vendored tree,
> `5c09d802abf73326a8c67110b308adb8b08e4c22`. Its documentation, model
> card, examples, and benchmarks remain in that private git source.

`crates/private/recall-engine/` is a pristine git subtree of
`crates/enzyme-core/` from the local enzyme-rust repository. Do not edit files
inside the subtree from Margins and do not apply downstream transforms. Margins
renames the package only at its dependency edge (`enzyme-core` as
`recall-engine`) and enables `embedder-compat` there.

The metadata file lives beside, rather than inside, the subtree so the subtree
remains byte-identical to upstream.

## Current pin

- Upstream repository: `https://github.com/byenzyme/enzyme-rust.git`
- Upstream revision: enzyme-rust master after stable evidence-document refs PR #15 and the role-blind shared-conversation forward-port PR #16 (merge `8650c07a97bf`) — no participant/speaker ownership grammar
- Upstream commit: `8650c07a97bfffdd42b0904373b47229c74a1e51`
- Upstream commit tree: `48a72635c940f8d79d0aedfae84165133fd6d653`
- Upstream subtree path: `crates/enzyme-core`
- Exact subtree tree: `ca5bd829a2adc77b22097af6f62654f2cb873326`
- Import method: byte-for-byte archive of the immutable merge commit and subtree path

PR #13 was synced from its exact reviewed commit before upstream merge or tag (previous pin: PR #12 head 32429cb177a639710b8120c8c791557ebb16e6aa).
No downstream transforms were applied. Future release syncs should continue to
use the tagged workflow below.

## Initial split and add used for v2

```bash
git -C /Users/example/Hacks/enzyme-rust \
  subtree split --prefix=crates/enzyme-core search-core-v2 \
  -b search-core-v2-split

git subtree add --prefix=crates/private/recall-engine \
  /Users/example/Hacks/enzyme-rust search-core-v2-split --squash
```

## Future tagged sync

After creating the next `search-core-vN` tag in enzyme-rust, run the following
from a clean Margins branch. Set `ENZYME_TAG` to that exact tag.

```bash
ENZYME_TAG=search-core-v3
SPLIT_BRANCH="${ENZYME_TAG}-split"

git -C /Users/example/Hacks/enzyme-rust \
  subtree split --prefix=crates/enzyme-core "$ENZYME_TAG" \
  -b "$SPLIT_BRANCH"

git subtree pull --prefix=crates/private/recall-engine \
  /Users/example/Hacks/enzyme-rust "$SPLIT_BRANCH" --squash
```

Then update the pin fields above and verify byte identity against the committed
tag (not the enzyme-rust working tree, which may contain ignored build files):

```bash
VERIFY_DIR="$(mktemp -d)"
git -C /Users/example/Hacks/enzyme-rust \
  archive "${ENZYME_TAG}:crates/enzyme-core" | tar -x -C "$VERIFY_DIR"
diff -ruN "$VERIFY_DIR" crates/private/recall-engine
```

An empty diff is required before committing the metadata update.
