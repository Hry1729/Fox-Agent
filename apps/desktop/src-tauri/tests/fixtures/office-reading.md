# Office reading fixtures

These files contain synthetic test data, not the user's AGV spreadsheet.

- `office-reading.xlsx` was generated with XlsxWriter. The worksheet filenames and workbook relationships were deliberately reordered. It exercises Chinese names, shared strings, numbers, booleans, a saved formula result and a formula without a saved result.
- `office-reading.pptx` was generated with python-pptx. Its slide XML filenames differ from presentation order. It exercises actual compressed OOXML packages and text extraction in presentation order.

The expected AGV duration of 123.5 and saved formula result of 247 are fixtures only. No formula is executed by Fox's readers.
