# Native sprite atlas page lifecycle

The desktop SDL atlas can opt a live page into a finalized state when its caller
knows that the page will receive no further inserts. Finalization is additive to
the existing `stasis_gfx_sprite_atlas_query_v1` record layout: page flag bit 5 is
`STASIS_SPRITE_ATLAS_PAGE_FLAG_SEALED`, and no V1 structure size or field offset
changes.

## Sealing contract

Call `stasis_gfx_sprite_atlas_seal_page_v1(snapshot_token, page_index)` with the
token and source page index returned by the same completed native query. The call
fails without changing live state when the token is stale, the page is not live,
the page is already sealed, or an atlas replacement plan is currently staged. A
successful transition bumps the atlas asset generation, which invalidates the
caller's snapshot token.

Sealing preserves the current texture allocation, resident coordinates, UVs,
reserved header, shelf cursor, and free-rectangle inventory. A sealed page is
reported as both `SEALED` and `PROTECTED`, never `PLAN_ELIGIBLE`. There is no
unseal operation within one renderer generation.

Allocation and release obey these rules:

- Page selection skips sealed pages, and the page-local reservation helper also
  rejects them defensively.
- Releasing a resident decrements the page's live-allocation count but does not
  publish a reusable free rectangle, reset a shelf or planner layout, destroy a
  dedicated texture, or recycle an empty sealed page. This includes a page whose
  only remaining pixels are the reserved white and missing-resource header.
- A same-allocation reload may upload replacement pixels into that resident's
  existing reserved rectangle. This does not add a new allocation or change UVs.
- A larger or otherwise incompatible reload allocates on an unsealed page and
  publishes the new resident only after upload succeeds. Allocation or upload
  failure leaves the old sealed resident live.

## Renderer generation and scope

Seals are device-local and renderer-generation scoped. Native renderer
invalidation discards every atlas page before rebuilding sprites from retained
sources, so restored pages begin dynamic and require a new query and explicit
seal. Surface-independent CPU asset identity remains unchanged.

This slice does not trim or replace GPU textures, repack residents, recalculate
UVs, or change atlas budgets. Those publication and evidence requirements belong
to the later trimmed-atlas work. The WebGL atlas and Android renderer remain on
their existing dynamic policies; they do not implement or emulate this native
desktop seal API.
