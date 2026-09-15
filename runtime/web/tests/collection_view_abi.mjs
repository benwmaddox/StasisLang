export const COLLECTION_VIEW_ABI_VERSION = 2;

export function collectionViewAbiGlobal(value = COLLECTION_VIEW_ABI_VERSION) {
  return new WebAssembly.Global({ value: "i32", mutable: false }, value);
}

export function installCollectionViewAbi(game, exports, {
  packageVersion = COLLECTION_VIEW_ABI_VERSION,
  wasmVersion = COLLECTION_VIEW_ABI_VERSION,
  includeWasmExport = true,
  omitPackageVersion = false,
} = {}) {
  if (!omitPackageVersion) game.collectionViewAbiVersion = packageVersion;
  if (includeWasmExport) exports.__stasis_collection_view_abi_version = collectionViewAbiGlobal(wasmVersion);
  return game;
}
