import { LiveCollection, LiveValue } from "./protocol";

export interface LiveValueTreeNode {
  kind: "group" | "value" | "collection" | "collection-row";
  label: string;
  path: string;
  value?: LiveValue;
  collection?: LiveCollection;
  rowIndex?: number;
  children: LiveValueTreeNode[];
}

function pathSegments(path: string): string[] {
  return path.split(".").filter((segment) => segment.length > 0);
}

function findOrAddGroup(children: LiveValueTreeNode[], label: string, path: string): LiveValueTreeNode {
  let node = children.find((candidate) => candidate.label === label && candidate.path === path);
  if (!node) {
    node = { kind: "group", label, path, children: [] };
    children.push(node);
  }
  return node;
}

function parentForPath(roots: LiveValueTreeNode[], path: string): LiveValueTreeNode[] {
  const segments = pathSegments(path);
  let children = roots;
  let currentPath = "";
  for (const segment of segments.slice(0, -1)) {
    currentPath = currentPath ? `${currentPath}.${segment}` : segment;
    children = findOrAddGroup(children, segment, currentPath).children;
  }
  return children;
}

function sortNodes(nodes: LiveValueTreeNode[]): void {
  nodes.sort((left, right) => left.label.localeCompare(right.label, undefined, { numeric: true }));
  for (const node of nodes) {
    sortNodes(node.children);
  }
}

export function buildLiveValueTree(
  values: readonly LiveValue[],
  collections: readonly LiveCollection[],
  tableCollections: ReadonlySet<string>,
  filterInactiveRows = true,
): LiveValueTreeNode[] {
  const roots: LiveValueTreeNode[] = [];
  const collectionPaths = collections.map((collection) => collection.path);

  for (const value of values) {
    if (collectionPaths.some((path) => value.path.startsWith(`${path}[`))) {
      continue;
    }
    const segments = pathSegments(value.path);
    parentForPath(roots, value.path).push({
      kind: "value",
      label: segments.at(-1) ?? value.path,
      path: value.path,
      value,
      children: [],
    });
  }

  for (const collection of collections) {
    const segments = pathSegments(collection.path);
    const collectionNode: LiveValueTreeNode = {
      kind: "collection",
      label: segments.at(-1) ?? collection.path,
      path: collection.path,
      collection,
      children: [],
    };
    const activeField = collection.fields.find((field) =>
      field.field.toLowerCase() === "active" && field.staticType === "bool",
    );
    const rows = filterInactiveRows && activeField
      ? collection.rows.filter((row) => row.values[activeField.field] === true)
      : collection.rows;
    for (const row of rows) {
      const rowPath = `${collection.path}[${row.index}]`;
      const rowNode: LiveValueTreeNode = {
        kind: "collection-row",
        label: `[${row.index}]`,
        path: rowPath,
        collection,
        rowIndex: row.index,
        children: [],
      };
      if (!tableCollections.has(collection.path)) {
        rowNode.children = collection.fields.map((field) => ({
          kind: "value",
          label: field.field || "value",
          path: field.field ? `${rowPath}.${field.field}` : rowPath,
          value: {
            path: field.field ? `${rowPath}.${field.field}` : rowPath,
            staticType: field.staticType,
            value: row.values[field.field],
            tick: collection.tick,
            watched: false,
          },
          children: [],
        }));
      }
      collectionNode.children.push(rowNode);
    }
    const parent = parentForPath(roots, collection.path);
    const existingGroup = parent.findIndex((node) =>
      node.kind === "group" && node.path === collection.path,
    );
    if (existingGroup >= 0) {
      collectionNode.children.push(...parent[existingGroup]!.children);
      parent.splice(existingGroup, 1);
    }
    parent.push(collectionNode);
  }

  sortNodes(roots);
  return roots;
}
