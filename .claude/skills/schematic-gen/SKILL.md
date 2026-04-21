---
name: schematic-gen
description: |
  Generate Virtuoso schematics from topology descriptions via vcli schematic commands.
  Use when: (1) user wants to draw/create a schematic in Virtuoso, (2) user says
  "draw the OTA" or "create the schematic", (3) after sizing is complete and ready
  to build the circuit, (4) user provides a topology and wants it instantiated.
argument-hint: [topology, e.g. "5T OTA" or "cascode current mirror"]
allowed-tools: Bash(vcli *) Read Write Edit
---

# Schematic Generation

Two-layer system: this skill provides topology knowledge, `vcli schematic` commands
provide the execution layer.

## CLI Commands

```bash
# Atomic operations
vcli schematic open  --lib L --cell C [--view schematic]
vcli schematic place --master lib/cell --name I --x X --y Y [--params w=14u,l=1u]
vcli schematic wire  --net N x1,y1 x2,y2 ...
vcli schematic conn  --net N --from I:T --to I:T
vcli schematic label --net N --x X --y Y
vcli schematic pin   --net N --type input --x X --y Y
vcli schematic check
vcli schematic save

# Batch from JSON spec
vcli schematic build --spec file.json
```

## JSON Spec Format

```json
{
  "target": { "lib": "myLib", "cell": "ota5t", "view": "schematic" },
  "instances": [
    { "name": "M5", "master": "smic13mmrf/p12", "x": 0, "y": 4,
      "params": { "w": "14u", "l": "1u" } }
  ],
  "connections": [
    { "net": "vtail", "from": "M5:D", "to": "M1:S" }
  ],
  "globals": [
    { "net": "vdd", "insts": ["M5:S", "M5:B", "M1:B"] }
  ],
  "pins": [
    { "net": "vip", "type": "input", "connect": "M2:G", "x": -4, "y": 2 }
  ]
}
```

## Workflow

1. **Collect sizing** — from gm/Id design or optimization results
2. **Generate JSON spec** — use topology template below, fill in W/L values
3. **Run build** — `vcli schematic build --spec spec.json`
4. **Verify** — `vcli schematic check`

## Topology Templates

### 5T OTA (PMOS Diff Pair + NMOS Mirror Load)

```
     VDD
      |
     M5 (PMOS tail, gate=nbias)
      |
    vtail
    /    \
  M1      M2  (PMOS diff pair)
  |        |
  nd1    vout
  |        |
  M3      M4  (NMOS mirror load, M3 diode)
  |        |
    GND   GND
```

Terminal mapping (SMIC 130nm MOSFET):
- PMOS `p12`: terminals D G S B (drain gate source bulk)
- NMOS `n12`: terminals D G S B

```json
{
  "target": { "lib": "myLib", "cell": "ota5t" },
  "instances": [
    { "name": "M5", "master": "smic13mmrf/p12", "x": 0,  "y": 8, "params": {} },
    { "name": "M1", "master": "smic13mmrf/p12", "x": -3, "y": 5, "params": {} },
    { "name": "M2", "master": "smic13mmrf/p12", "x": 3,  "y": 5, "params": {} },
    { "name": "M3", "master": "smic13mmrf/n12", "x": -3, "y": 2, "params": {} },
    { "name": "M4", "master": "smic13mmrf/n12", "x": 3,  "y": 2, "params": {} }
  ],
  "connections": [
    { "net": "vtail", "from": "M5:D", "to": "M1:S" },
    { "net": "vtail", "from": "M5:D", "to": "M2:S" },
    { "net": "nd1",   "from": "M1:D", "to": "M3:D" },
    { "net": "nd1",   "from": "M3:D", "to": "M3:G" },
    { "net": "nd1",   "from": "M3:G", "to": "M4:G" },
    { "net": "vout",  "from": "M2:D", "to": "M4:D" }
  ],
  "globals": [
    { "net": "vdd!", "insts": ["M5:S", "M5:B", "M1:B", "M2:B"] },
    { "net": "gnd!", "insts": ["M3:S", "M3:B", "M4:S", "M4:B"] }
  ],
  "pins": [
    { "net": "vip",   "type": "input",  "x": 6,  "y": 5 },
    { "net": "vin",   "type": "input",  "x": -6, "y": 5 },
    { "net": "nbias", "type": "input",  "x": -2, "y": 8 },
    { "net": "vout",  "type": "output", "x": 6,  "y": 2 }
  ]
}
```

### Folded-Cascode OTA (PMOS Input)

```
     VDD
    / | \
  M5  M9  M10  (PMOS current sources)
  |   |    |
vtail M7   M8   (PMOS cascode)
 / \  |    |
M1  M2 |   vout
|    | |    |
M3  M4 M11  M12  (NMOS cascode)
|    |  |    |
M13 M14 M15  M16  (NMOS current source)
     GND
```

Additional instances: bias mirrors, cascode devices. Fill params from sizing.

### Two-Stage Miller OTA

```
Stage 1:          Stage 2:
M5 (tail)         M7 (output NMOS)
 / \                |
M1  M2            vout---Cc---nd1
|    |              |
M3  M4            M6 (output load)
```

Compensation: Cc between nd1 and vout, optional Rz in series.

## Coordinate Conventions

- Schematic coordinates in microns
- Y increases upward (VDD at top, GND at bottom)
- Symmetric pairs: mirror around x=0
- Typical spacing: 3 units horizontal, 3 units vertical between rows
- Pins placed at the edges (x = +-6 or more)

## PDK Master Names

| PDK | NMOS | PMOS |
|-----|------|------|
| SMIC 130nm (smic13mmrf) | `n12` | `p12` |
| TSMC 180nm | `nch` | `pch` |
| Sky130 | `sky130_fd_pr/nfet_01v8` | `sky130_fd_pr/pfet_01v8` |

Params are PDK-specific. Always pass W/L via `--params` or JSON `params` field.

## Notes

- `vcli schematic open` must be called before other commands (opens cellview in GUI)
- `conn` uses `schCreateWire` for auto-routing between terminals
- `wire` uses exact coordinates — use when you need specific routing paths
- Global nets (vdd!, gnd!) use `!` suffix in Virtuoso convention
- After `build`, always run `check` and `save`
