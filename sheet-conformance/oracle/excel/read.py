#!/usr/bin/env python3
# paged.sheet — Excel oracle lane, step 3: read what Excel computed.
#
# Usage:  python3 read.py [family ...]
# Input:  build/<family>.excel.xlsx  (Excel's own re-save, from drive.sh)
# Output: recorded/<family>.tsv      (committed; the Rust lane reads it in CI)
#
# Columns: file, id, formula, type, value
#   file/id/formula  the golden case, copied so the Rust lane can prove the
#                    recording still matches the live corpus (a changed
#                    formula is a STALE row, never a silent pass)
#   type             n (number) | s (text) | b (bool) | e (error) | blank
#                    | rejected (excel-rejects.tsv) | unrepresentable (setup)
#   value            Excel's cached <v> VERBATIM — numbers keep the 17-digit
#                    text Excel wrote, text is escaped (\\ \t \n \r)
#
# The host cell is located through workbook.xml -> rels -> part, by SHEET NAME
# (Excel renumbers the parts on save), never by part index.

from __future__ import annotations

import sys
import zipfile
import xml.etree.ElementTree as ET

import oracle_cases as oc

M = "{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"
R = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
PR = "{http://schemas.openxmlformats.org/package/2006/relationships}"


def esc(s: str) -> str:
    return (s.replace("\\", "\\\\").replace("\t", "\\t")
            .replace("\n", "\\n").replace("\r", "\\r"))


def shared_strings(z: zipfile.ZipFile) -> list[str]:
    try:
        root = ET.fromstring(z.read("xl/sharedStrings.xml"))
    except KeyError:
        return []
    out = []
    for si in root.findall(f"{M}si"):
        # Plain <t> or rich runs <r><t>; phonetic <rPh> runs are not text.
        parts = []
        for child in si:
            if child.tag == f"{M}t":
                parts.append(child.text or "")
            elif child.tag == f"{M}r":
                parts.extend(t.text or "" for t in child.findall(f"{M}t"))
        out.append("".join(parts))
    return out


def sheet_parts(z: zipfile.ZipFile) -> dict[str, str]:
    wb = ET.fromstring(z.read("xl/workbook.xml"))
    rels = ET.fromstring(z.read("xl/_rels/workbook.xml.rels"))
    target = {r.get("Id"): r.get("Target") for r in rels.findall(f"{PR}Relationship")}
    out = {}
    for s in wb.find(f"{M}sheets"):
        t = target[s.get(f"{R}id")].lstrip("/")
        out[s.get("name")] = t if t.startswith("xl/") else "xl/" + t
    return out


XLRD = "{http://schemas.microsoft.com/office/spreadsheetml/2017/richdata}"

# Excel stores the errors that post-date the file format (#SPILL!, #CALC!,
# #BLOCKED!, ...) as `t="e"` with the LEGACY text "#VALUE!" plus a `vm`
# value-metadata index pointing at a rich `_error` value whose errorType says
# what the error really is. Reading `<v>` alone records #VALUE! for a #CALC!
# — which would credit the engine with "agreeing" on SEQUENCE(0) because it
# grounds #CALC! to #VALUE!. Decode the rich value instead.
# errorType is 0-based in the legacy order, then the newer errors. Checked
# against this run: 2 came back on VLOOKUP(..,0,..) (#VALUE!), 13 on
# SEQUENCE(0) (#CALC!).
RICH_ERRORS = dict(enumerate([
    "#NULL!", "#DIV/0!", "#VALUE!", "#REF!", "#NAME?", "#NUM!", "#N/A",
    "#GETTING_DATA", "#SPILL!", "#CONNECT!", "#BLOCKED!", "#UNKNOWN!",
    "#FIELD!", "#CALC!", "#BUSY!"]))


def rich_errors(z: zipfile.ZipFile) -> dict[int, str]:
    """vm index (1-based) -> the rich error token, for every rich error."""
    try:
        meta = ET.fromstring(z.read("xl/metadata.xml"))
        rv = ET.fromstring(z.read("xl/richData/rdrichvalue.xml"))
        st = ET.fromstring(z.read("xl/richData/rdrichvaluestructure.xml"))
    except KeyError:
        return {}
    structs = st.findall(f"{XLRD}s")
    values = rv.findall(f"{XLRD}rv")
    types = [t.get("name") for t in meta.find(f"{M}metadataTypes")]
    fut = {f.get("name"): f.findall(f"{M}bk") for f in meta.findall(f"{M}futureMetadata")}
    out = {}
    vmeta = meta.find(f"{M}valueMetadata")
    if vmeta is None:
        return {}
    for n, bk in enumerate(vmeta.findall(f"{M}bk"), 1):
        rc = bk.find(f"{M}rc")
        if types[int(rc.get("t")) - 1] != "XLRICHVALUE":
            continue
        rvb = fut["XLRICHVALUE"][int(rc.get("v"))].find(f".//{XLRD}rvb")
        val = values[int(rvb.get("i"))]
        s = structs[int(val.get("s"))]
        if s.get("t") != "_error":
            continue
        keys = [k.get("n") for k in s.findall(f"{XLRD}k")]
        fields = dict(zip(keys, (v.text for v in val.findall(f"{XLRD}v"))))
        et = int(fields.get("errorType", -1))
        out[n] = RICH_ERRORS.get(et, f"#RICH-ERROR-{et}")
    return out


def read_cell(z, part: str, addr: str, sst: list[str],
              rich: dict[int, str] | None = None) -> tuple[str, str]:
    root = ET.fromstring(z.read(part))
    for c in root.iter(f"{M}c"):
        if c.get("r") != addr:
            continue
        t = c.get("t", "n")
        v = c.find(f"{M}v")
        if t == "inlineStr":
            return "s", "".join(x.text or "" for x in c.iter(f"{M}t"))
        if v is None:
            return "blank", ""
        text = v.text or ""
        if t == "s":
            return "s", sst[int(text)]
        if t == "str":
            return "s", text
        if t == "e" and c.get("vm") and rich and int(c.get("vm")) in rich:
            return "e", rich[int(c.get("vm"))]
        if t in ("b", "e", "n"):
            return t, text
        raise SystemExit(f"{part}!{addr}: unknown cell type {t!r}")
    return "blank", ""


def read_family(family: str) -> int:
    cases = oc.load_family(family)
    src = oc.build_dir() / f"{family}.excel.xlsx"
    if not src.exists():
        raise SystemExit(f"{src} missing — run drive.sh first")
    out_dir = oc.oracle_dir() / "recorded"
    out_dir.mkdir(exist_ok=True)
    src_dir = ("corpus/format-corpus (as TEXT() probes)" if family == oc.FORMAT_FAMILY
               else f"corpus/fn-corpus/{family}")
    rows = [f"# Excel-recorded values for {src_dir} (generated by "
            "oracle/excel/read.py — do not hand-edit)",
            "# file\tid\tformula\ttype\tvalue"]
    with zipfile.ZipFile(src) as z:
        sst = shared_strings(z)
        parts = sheet_parts(z)
        rich = rich_errors(z)
        pt, pv = read_cell(z, parts[oc.LOCALE_PROBE_SHEET], "A1", sst)
        if (pt, pv) != ("s", oc.LOCALE_PROBE_EXPECTED):
            raise SystemExit(
                f"{src.name}: locale probe came back {pv!r}, want "
                f"{oc.LOCALE_PROBE_EXPECTED!r} — Excel did not compute in en-US, "
                "refusing to record (drive.sh pins the locale; was it bypassed?)")
        rejected = oc.rejected_keys()
        for c in cases:
            why = oc.unrepresentable(c)
            if c.key in rejected:
                t, v = "rejected", "Excel refuses to enter this formula"
            elif why:
                t, v = "unrepresentable", why
            else:
                t, v = read_cell(z, parts[c.sheet], c.host, sst, rich)
            rows.append("\t".join([c.file, c.id, esc(c.formula), t, esc(v)]))
    (out_dir / f"{family}.tsv").write_text("\n".join(rows) + "\n", encoding="utf-8")
    print(f"{family}: recorded {len(cases)} case(s)")
    return len(cases)


def main(argv: list[str]) -> int:
    fams = argv or oc.families()
    total = sum(read_family(f) for f in fams)
    print(f"recorded {total} case(s) from {len(fams)} workbook(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
