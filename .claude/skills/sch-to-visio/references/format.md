# Input and output

Input is the JSON emitted by `vcli schematic export`, containing `format`, `version`, `instances`, `nets`, and `pins`. The script writes a ZIP-based `.vsdx` package with one page and editable generic shapes. PDK-specific artwork and exact wire endpoints require extending the vcli export schema with symbol geometry and terminal coordinates.
