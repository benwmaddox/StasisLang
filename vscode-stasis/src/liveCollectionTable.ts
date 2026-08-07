import { displayRuntimeValue, LiveCollection } from "./protocol";

export interface LiveCollectionTableColumn {
  readonly key: string;
  readonly label: string;
  readonly staticType: string;
}

export interface LiveCollectionTableRow {
  readonly index: number;
  readonly cells: readonly string[];
}

export interface LiveCollectionTableModel {
  readonly path: string;
  readonly elementShape: string;
  readonly tick: number;
  readonly capacity: number;
  readonly activeCount: number;
  readonly rowsTruncated: boolean;
  readonly filtered: boolean;
  readonly columns: readonly LiveCollectionTableColumn[];
  readonly rows: readonly LiveCollectionTableRow[];
}

export function buildLiveCollectionTableModel(
  collection: LiveCollection,
  filterInactiveRows: boolean,
): LiveCollectionTableModel {
  const activeField = collection.fields.find(
    (field) => field.field.toLowerCase() === "active" && field.staticType === "bool",
  );
  const rows = filterInactiveRows && activeField
    ? collection.rows.filter((row) => row.values[activeField.field] === true)
    : collection.rows;

  return {
    path: collection.path,
    elementShape: collection.elementShape,
    tick: collection.tick,
    capacity: collection.capacity,
    activeCount: collection.activeCount,
    rowsTruncated: collection.rowsTruncated,
    filtered: filterInactiveRows && activeField !== undefined,
    columns: collection.fields.map((field) => ({
      key: field.field,
      label: field.field || "value",
      staticType: field.staticType,
    })),
    rows: rows.map((row) => ({
      index: row.index,
      cells: collection.fields.map((field) => displayRuntimeValue(row.values[field.field])),
    })),
  };
}
