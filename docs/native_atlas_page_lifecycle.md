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

Sealing preserves the page width and every resident's allocation and content
coordinates. It derives the occupied bottom from the six-pixel runtime header
and every live padded allocation, then chooses the smallest supported
power-of-two height, with a 64-pixel minimum, that covers that bottom. If the
current height is already the smallest supported extent, sealing changes no
texture dimensions. Empty or header-only pages deterministically select the
64-pixel minimum when their current extent permits it.

When the height shrinks, the runtime builds a complete replacement texture while
the old texture remains live. Every resident source must still decode to the
published dimensions and raster hash at the same file modification time. The
replacement receives the same header pixels and padded resident uploads at the
same x/y coordinates. Only after every upload and a final snapshot-token check
does one publication update the page texture and height, resident texture
references, and cached normalized V coordinates. Allocation, decode, source
drift, upload, or generation failure destroys the candidate and leaves the old
page, residents, UVs, token, and asset generation unchanged.

A sealed page is reported as both `SEALED` and `PROTECTED`, never
`PLAN_ELIGIBLE`. The unchanged V1 query reports the realized width, height, and
RGBA8 allocation bytes after publication; no structure size or field offset
changes. There is no unseal operation within one renderer generation.

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

The WebGL atlas and Android renderer remain on their existing dynamic policies;
they do not implement or emulate this native desktop seal API.

## Byte accounting

The V1 query reports nominal RGBA8 device bytes as `width * height * 4` without
mipmaps. The lifecycle fixture covers these exact cases:

| Transition | Old bytes | Final bytes | Device bytes avoided |
| --- | ---: | ---: | ---: |
| 512x512 -> 512x256 | 1,048,576 | 524,288 | 524,288 |
| 2048x2048 -> 2048x128 | 16,777,216 | 1,048,576 | 15,728,640 |

Publication temporarily retains both GPU textures, so nominal device peak is
old plus final allocation: 1,572,864 bytes and 17,825,792 bytes respectively.
Texture initialization uploads the complete final allocation, followed by 32
header bytes and each live padded resident. These upload bytes are distinct from
PNG compressed size. The fixture's 512-page replacement uploads 691,588 bytes;
the 2048-page replacement uploads 1,090,224 bytes. The largest CPU staging
allocation is the greater of the zero-filled final texture image and one decoded
raster plus its padded upload. SDL/backend alignment and driver-private copies
are not exposed by this API, so these are deterministic nominal RGBA8 counts,
not claims about unobservable driver allocation.
