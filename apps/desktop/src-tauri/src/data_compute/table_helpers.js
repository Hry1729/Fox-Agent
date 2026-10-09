// Shared by whole and chunked QuickJS wrappers. No I/O bindings are added.
const dateKey = (value) => {
  if (value instanceof Date) {
    if (!Number.isFinite(value.getTime()) || value.getUTCFullYear() < 1 || value.getUTCFullYear() > 9999) throw new Error("dateKey requires a valid Date within 0001..9999");
    return value.toISOString().slice(0, 10);
  }
  if (value && typeof value === "object" && !Array.isArray(value)) {
    const keys = Object.keys(value);
    if (keys.length !== 2 || value.type !== "date" || !keys.includes("value")) {
      throw new Error("dateKey rejects ordinary objects; use an explicit date value");
    }
    value = value.value;
  }
  if (typeof value !== "string" || !/^\d{4}-\d{2}-\d{2}(?:T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?(?:Z|[+-]\d{2}:\d{2})?)?$/.test(value)) {
    throw new Error("dateKey requires YYYY-MM-DD or an explicit ISO timestamp; numbers are not guessed as dates");
  }
  const year = Number(value.slice(0, 4));
  const month = Number(value.slice(5, 7));
  const day = Number(value.slice(8, 10));
  const calendar = new Date(0);
  calendar.setUTCFullYear(year, month - 1, day);
  if (year < 1 || calendar.getUTCFullYear() !== year || calendar.getUTCMonth() !== month - 1 || calendar.getUTCDate() !== day) {
    throw new Error("dateKey received an invalid calendar date");
  }
  if (value.length === 10) return value;
  if (Number(value.slice(11, 13)) > 23 || Number(value.slice(14, 16)) > 59 || Number(value.slice(17, 19)) > 59) {
    throw new Error("dateKey received an invalid ISO clock time");
  }
  if (!/(?:Z|[+-]\d{2}:\d{2})$/.test(value)) return value.slice(0, 10);
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) throw new Error("dateKey received an invalid ISO timestamp");
  return dateKey(new Date(timestamp));
};

const scalarKey = (value) => {
  if (value instanceof Date) return dateKey(value);
  if (typeof value === "string") {
    if (value.length === 0) throw new Error("scalarKey requires a nonempty key");
    return value;
  }
  if (typeof value === "number" && Number.isFinite(value) || typeof value === "boolean") return value;
  throw new Error("scalarKey rejects null, undefined, arrays, objects and non-finite numbers");
};

const saveTable = (name, spec) => {
  if (!spec || typeof spec !== "object" || !Array.isArray(spec.columns) || !Array.isArray(spec.rows)) {
    throw new Error("saveTable requires {columns, rows, keyColumns?, dateColumns?, source?}");
  }
  const columns = spec.columns;
  if (columns.length === 0 || columns.length > 512 || columns.some(column => typeof column !== "string" || !column.trim()) || new Set(columns).size !== columns.length) {
    throw new Error("saveTable requires 1..512 unique nonempty column names");
  }
  const keyColumns = spec.keyColumns === undefined ? [] : spec.keyColumns;
  const dateColumns = spec.dateColumns === undefined ? [] : spec.dateColumns;
  for (const selected of [keyColumns, dateColumns]) {
    if (!Array.isArray(selected) || new Set(selected).size !== selected.length || selected.some(column => !columns.includes(column))) {
      throw new Error("saveTable key/date columns must be unique declared column names");
    }
  }
  const rows = spec.rows.map((row, rowIndex) => {
    if (!Array.isArray(row) || row.length !== columns.length) throw new Error("saveTable row width differs from columns at row " + rowIndex);
    return row.map((value, columnIndex) => {
      const column = columns[columnIndex];
      if (value === null || value === undefined) {
        if (keyColumns.includes(column)) throw new Error("saveTable key is empty at row " + rowIndex + ", column " + column);
        return null;
      }
      if (dateColumns.includes(column)) return dateKey(value);
      if (keyColumns.includes(column)) return scalarKey(value);
      if (value instanceof Date) return dateKey(value);
      if (["string", "boolean"].includes(typeof value) || typeof value === "number" && Number.isFinite(value)) return value;
      throw new Error("saveTable cells must be scalar at row " + rowIndex + ", column " + column);
    });
  });
  __foxSavedFiles.push({ name: String(name), format: "table", spec: { columns, rows, keyColumns, dateColumns, source: spec.source === undefined ? null : spec.source } });
  return { name: String(name) };
};
globalThis.scalarKey = scalarKey;
globalThis.dateKey = dateKey;
globalThis.saveTable = saveTable;
