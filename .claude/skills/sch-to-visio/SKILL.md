---
name: sch-to-visio
description: Convert a vcli schematic export JSON document into an editable Microsoft Visio .vsdx file. Use for schematic visualization and document export; requires exported schematic data and does not connect to Virtuoso by itself.
---

# Schematic to Visio

Convert JSON from `vcli schematic export` into a Visio drawing using the bundled standard-library Python script.

```bash
vcli schematic export --output schematic-export.json
python3 .claude/skills/sch-to-visio/scripts/json_to_vsdx.py schematic-export.json schematic.vsdx
```

The first version creates editable generic shapes, text labels, and a page in a `.vsdx` package. It preserves instance names, masters, coordinates, nets, and pins. It does not reproduce PDK-specific symbol artwork or exact wire endpoints because those fields are not present in the current export schema. See [references/format.md](references/format.md).
