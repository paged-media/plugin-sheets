#!/usr/bin/env python3
# Reproducible XLSX fixtures for sheet-xlsx conformance (spec §10).
#
# stdlib only (zipfile + hand-authored XML strings; no openpyxl/xlsxwriter)
# so the corpus is auditable byte-for-byte and has no external deps. Each
# fixture targets a specific parse/preservation feature; the docstring on
# each builder records what it exercises. Run:
#
#     python3 corpus/xlsx-corpus/generate.py
#
# Regenerates all six *.xlsx in this directory deterministically (fixed zip
# member order + a fixed timestamp so the bytes are stable across runs).

import os
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
# Fixed DOS timestamp (1980-01-01 00:00:00) so re-running is byte-stable.
FIXED_DATE = (1980, 1, 1, 0, 0, 0)

XML_DECL = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n'
NS_MAIN = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
NS_R = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
NS_CT = "http://schemas.openxmlformats.org/package/2006/content-types"
NS_REL = "http://schemas.openxmlformats.org/package/2006/relationships"

CT_WORKBOOK = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"
CT_WORKSHEET = "application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"
CT_SHARED = "application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"
CT_STYLES = "application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"
CT_CALCCHAIN = "application/vnd.openxmlformats-officedocument.spreadsheetml.calcChain+xml"
CT_RELS = "application/vnd.openxmlformats-package.relationships+xml"
CT_XML = "application/xml"
CT_VML = "application/vnd.openxmlformats-officedocument.vmlDrawing"

RT_OFFICE_DOC = NS_R + "/officeDocument"
RT_WORKSHEET = NS_R + "/worksheet"
RT_SHARED = NS_R + "/sharedStrings"
RT_STYLES = NS_R + "/styles"
RT_CALCCHAIN = NS_R + "/calcChain"


def write_zip(path, members):
    """Write `members` (list of (name, str_body)) in order, deterministically."""
    full = os.path.join(HERE, path)
    with zipfile.ZipFile(full, "w", zipfile.ZIP_DEFLATED) as z:
        for name, body in members:
            info = zipfile.ZipInfo(name, date_time=FIXED_DATE)
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, body)
    print("wrote", path)


def content_types(overrides):
    """overrides: list of (partname, contenttype)."""
    s = XML_DECL + f'<Types xmlns="{NS_CT}">'
    s += f'<Default Extension="rels" ContentType="{CT_RELS}"/>'
    s += f'<Default Extension="xml" ContentType="{CT_XML}"/>'
    for pn, ct in overrides:
        s += f'<Override PartName="{pn}" ContentType="{ct}"/>'
    s += "</Types>"
    return s


def root_rels():
    return (
        XML_DECL
        + f'<Relationships xmlns="{NS_REL}">'
        + f'<Relationship Id="rId1" Type="{RT_OFFICE_DOC}" Target="xl/workbook.xml"/>'
        + "</Relationships>"
    )


def workbook(sheets, date1904=False, defined_names=None):
    """sheets: list of (name, sheetId, rId). defined_names: list of (name, text, localSheetId|None)."""
    s = XML_DECL
    s += f'<workbook xmlns="{NS_MAIN}" xmlns:r="{NS_R}">'
    if date1904:
        s += '<workbookPr date1904="1"/>'
    s += "<sheets>"
    for name, sid, rid in sheets:
        s += f'<sheet name="{name}" sheetId="{sid}" r:id="{rid}"/>'
    s += "</sheets>"
    if defined_names:
        s += "<definedNames>"
        for nm, text, local in defined_names:
            if local is None:
                s += f'<definedName name="{nm}">{text}</definedName>'
            else:
                s += f'<definedName name="{nm}" localSheetId="{local}">{text}</definedName>'
        s += "</definedNames>"
    s += "</workbook>"
    return s


def workbook_rels(rels):
    """rels: list of (rId, type, target)."""
    s = XML_DECL + f'<Relationships xmlns="{NS_REL}">'
    for rid, ty, tgt in rels:
        s += f'<Relationship Id="{rid}" Type="{ty}" Target="{tgt}"/>'
    s += "</Relationships>"
    return s


def shared_strings(items):
    """items: list of str OR list-of-runs (list of str) for rich text."""
    s = XML_DECL
    s += f'<sst xmlns="{NS_MAIN}" count="{len(items)}" uniqueCount="{len(items)}">'
    for it in items:
        if isinstance(it, list):
            s += "<si>"
            for run in it:
                s += f"<r><t>{run}</t></r>"
            s += "</si>"
        else:
            s += f"<si><t>{it}</t></si>"
    s += "</sst>"
    return s


def styles(num_fmts=None, cell_xfs=None):
    """num_fmts: list of (id, code). cell_xfs: list of dicts with numFmtId/fontId/fillId/borderId."""
    num_fmts = num_fmts or []
    cell_xfs = cell_xfs or [dict(numFmtId=0, fontId=0, fillId=0, borderId=0)]
    s = XML_DECL + f'<styleSheet xmlns="{NS_MAIN}">'
    if num_fmts:
        s += f'<numFmts count="{len(num_fmts)}">'
        for i, code in num_fmts:
            s += f'<numFmt numFmtId="{i}" formatCode="{code}"/>'
        s += "</numFmts>"
    s += '<fonts count="2"><font><sz val="11"/><name val="Calibri"/></font>'
    s += '<font><b/><sz val="11"/><name val="Calibri"/></font></fonts>'
    s += '<fills count="3"><fill><patternFill patternType="none"/></fill>'
    s += '<fill><patternFill patternType="gray125"/></fill>'
    s += '<fill><patternFill patternType="solid"><fgColor rgb="FFFFFF00"/></patternFill></fill></fills>'
    s += '<borders count="2"><border><left/><right/><top/><bottom/><diagonal/></border>'
    s += '<border><left style="thin"/><right style="thin"/><top style="thin"/><bottom style="thin"/><diagonal/></border></borders>'
    s += '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
    s += f'<cellXfs count="{len(cell_xfs)}">'
    for xf in cell_xfs:
        s += (
            f'<xf numFmtId="{xf["numFmtId"]}" fontId="{xf["fontId"]}" '
            f'fillId="{xf["fillId"]}" borderId="{xf["borderId"]}" xfId="0" applyNumberFormat="1"/>'
        )
    s += "</cellXfs>"
    s += "</styleSheet>"
    return s


def calc_chain(cells):
    """cells: list of (ref, sheetId)."""
    s = XML_DECL + f'<calcChain xmlns="{NS_MAIN}">'
    for ref, sid in cells:
        s += f'<c r="{ref}" i="{sid}"/>'
    s += "</calcChain>"
    return s


# --- worksheet body builders -------------------------------------------------

def ws(dimension, rows_xml, extras_before="", extras_after="", merges=None, cols=None):
    s = XML_DECL + f'<worksheet xmlns="{NS_MAIN}" xmlns:r="{NS_R}">'
    s += extras_before
    if dimension:
        s += f'<dimension ref="{dimension}"/>'
    if cols:
        s += "<cols>" + cols + "</cols>"
    s += "<sheetData>" + rows_xml + "</sheetData>"
    if merges:
        s += f'<mergeCells count="{len(merges)}">'
        for m in merges:
            s += f'<mergeCell ref="{m}"/>'
        s += "</mergeCells>"
    s += extras_after
    s += "</worksheet>"
    return s


# --- fixtures ---------------------------------------------------------------

def gen_01_minimal():
    """01: 3 cells, inline values only (number, inlineStr, number). No
    sharedStrings/styles parts at all — the smallest valid workbook."""
    rows = (
        '<row r="1">'
        '<c r="A1" t="n"><v>1</v></c>'
        '<c r="B1" t="inlineStr"><is><t>hello</t></is></c>'
        '<c r="C1" t="n"><v>3.14</v></c>'
        "</row>"
    )
    members = [
        ("[Content_Types].xml", content_types([
            ("/xl/workbook.xml", CT_WORKBOOK),
            ("/xl/worksheets/sheet1.xml", CT_WORKSHEET),
        ])),
        ("_rels/.rels", root_rels()),
        ("xl/workbook.xml", workbook([("Sheet1", 1, "rId1")])),
        ("xl/_rels/workbook.xml.rels", workbook_rels([
            ("rId1", RT_WORKSHEET, "worksheets/sheet1.xml"),
        ])),
        ("xl/worksheets/sheet1.xml", ws("A1:C1", rows)),
    ]
    write_zip("01-minimal.xlsx", members)


def gen_02_formulas():
    """02: formulas with cached values + sharedStrings + a calcChain.
    A1=2, A2=3, A3=SUM(A1:A2)=5, B1 is a shared string label."""
    rows = (
        '<row r="1"><c r="A1" t="n"><v>2</v></c>'
        '<c r="B1" t="s"><v>0</v></c></row>'
        '<row r="2"><c r="A2" t="n"><v>3</v></c>'
        '<c r="B2" t="s"><v>1</v></c></row>'
        '<row r="3"><c r="A3" t="n"><f>SUM(A1:A2)</f><v>5</v></c>'
        '<c r="B3" t="str"><f>B1&amp;B2</f><v>SumProduct</v></c></row>'
    )
    members = [
        ("[Content_Types].xml", content_types([
            ("/xl/workbook.xml", CT_WORKBOOK),
            ("/xl/worksheets/sheet1.xml", CT_WORKSHEET),
            ("/xl/sharedStrings.xml", CT_SHARED),
            ("/xl/calcChain.xml", CT_CALCCHAIN),
        ])),
        ("_rels/.rels", root_rels()),
        ("xl/workbook.xml", workbook([("Sheet1", 1, "rId1")])),
        ("xl/_rels/workbook.xml.rels", workbook_rels([
            ("rId1", RT_WORKSHEET, "worksheets/sheet1.xml"),
            ("rId2", RT_SHARED, "sharedStrings.xml"),
            ("rId3", RT_CALCCHAIN, "calcChain.xml"),
        ])),
        ("xl/sharedStrings.xml", shared_strings(["Sum", "Product"])),
        ("xl/worksheets/sheet1.xml", ws("A1:B3", rows)),
        ("xl/calcChain.xml", calc_chain([("A3", 1), ("B3", 1)])),
    ]
    write_zip("02-formulas.xlsx", members)


def gen_03_styles():
    """03: custom numFmts (id>=164) + built-in numFmt ids + merges + col
    widths + custom row heights. Exercises the styles + geometry parse."""
    num_fmts = [(164, '&quot;$&quot;#,##0.00'), (165, "0.000%")]
    cell_xfs = [
        dict(numFmtId=0, fontId=0, fillId=0, borderId=0),   # 0: General
        dict(numFmtId=2, fontId=1, fillId=0, borderId=1),   # 1: built-in 0.00, bold, border
        dict(numFmtId=164, fontId=0, fillId=2, borderId=0), # 2: custom $#,##0.00, yellow fill
        dict(numFmtId=9, fontId=0, fillId=0, borderId=0),   # 3: built-in 0%
        dict(numFmtId=165, fontId=0, fillId=0, borderId=0), # 4: custom 0.000%
    ]
    rows = (
        '<row r="1" ht="28.5" customHeight="1">'
        '<c r="A1" s="1" t="n"><v>1234.5</v></c>'
        '<c r="B1" s="2" t="n"><v>9999.99</v></c>'
        '<c r="C1" s="3" t="n"><v>0.25</v></c>'
        "</row>"
        '<row r="2">'
        '<c r="A2" s="4" t="n"><v>0.12345</v></c>'
        "</row>"
    )
    cols = (
        '<col min="1" max="1" width="18.5" customWidth="1"/>'
        '<col min="2" max="3" width="12.0" customWidth="1"/>'
    )
    members = [
        ("[Content_Types].xml", content_types([
            ("/xl/workbook.xml", CT_WORKBOOK),
            ("/xl/worksheets/sheet1.xml", CT_WORKSHEET),
            ("/xl/styles.xml", CT_STYLES),
        ])),
        ("_rels/.rels", root_rels()),
        ("xl/workbook.xml", workbook([("Sheet1", 1, "rId1")])),
        ("xl/_rels/workbook.xml.rels", workbook_rels([
            ("rId1", RT_WORKSHEET, "worksheets/sheet1.xml"),
            ("rId2", RT_STYLES, "styles.xml"),
        ])),
        ("xl/styles.xml", styles(num_fmts, cell_xfs)),
        ("xl/worksheets/sheet1.xml", ws("A1:C2", rows, merges=["A1:A2"], cols=cols)),
    ]
    write_zip("03-styles.xlsx", members)


def gen_04_unknown_parts():
    """04: a customXml part + a fake vbaProject.bin + a calcChain. The
    customXml + vba bytes MUST survive byte-identical (preservation of
    unknown PARTS); calcChain MUST drop on save."""
    rows = '<row r="1"><c r="A1" t="n"><v>7</v></c></row>'
    fake_vba = "MZ\x00\x00fake-vba-project-bytes-\x01\x02\x03not-executed"
    custom_xml = '<?xml version="1.0"?><myData xmlns="urn:example:custom"><field>preserved</field></myData>'
    members = [
        ("[Content_Types].xml", content_types([
            ("/xl/workbook.xml", CT_WORKBOOK),
            ("/xl/worksheets/sheet1.xml", CT_WORKSHEET),
            ("/xl/calcChain.xml", CT_CALCCHAIN),
            ("/customXml/item1.xml", CT_XML),
            ("/xl/vbaProject.bin", "application/vnd.ms-office.vbaProject"),
        ])),
        ("_rels/.rels", root_rels()),
        ("xl/workbook.xml", workbook([("Sheet1", 1, "rId1")])),
        ("xl/_rels/workbook.xml.rels", workbook_rels([
            ("rId1", RT_WORKSHEET, "worksheets/sheet1.xml"),
            ("rId3", RT_CALCCHAIN, "calcChain.xml"),
        ])),
        ("customXml/item1.xml", custom_xml),
        ("xl/vbaProject.bin", fake_vba),
        ("xl/worksheets/sheet1.xml", ws("A1:A1", rows)),
        ("xl/calcChain.xml", calc_chain([("A1", 1)])),
    ]
    write_zip("04-unknown-parts.xlsx", members)


def gen_05_unknown_subtrees():
    """05: a worksheet with unknown <worksheet> children (sheetPr before
    sheetData, conditionalFormatting + extLst after) + unknown per-cell
    attrs. The subtrees survive a dirty re-encode; per-cell unknown attrs
    survive only via lazy-verbatim (T0 granularity)."""
    rows = (
        '<row r="1">'
        '<c r="A1" t="n" customUnknownAttr="keepme"><v>10</v></c>'
        '<c r="B1" t="n"><v>20</v></c>'
        "</row>"
    )
    before = '<sheetPr><tabColor rgb="FFFF0000"/></sheetPr>'
    after = (
        '<conditionalFormatting sqref="A1:B1">'
        '<cfRule type="cellIs" dxfId="0" priority="1" operator="greaterThan"><formula>5</formula></cfRule>'
        "</conditionalFormatting>"
        '<pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/>'
        '<extLst><ext uri="{EXAMPLE-EXT}"><x:future xmlns:x="urn:x"/></ext></extLst>'
    )
    members = [
        ("[Content_Types].xml", content_types([
            ("/xl/workbook.xml", CT_WORKBOOK),
            ("/xl/worksheets/sheet1.xml", CT_WORKSHEET),
        ])),
        ("_rels/.rels", root_rels()),
        ("xl/workbook.xml", workbook([("Sheet1", 1, "rId1")])),
        ("xl/_rels/workbook.xml.rels", workbook_rels([
            ("rId1", RT_WORKSHEET, "worksheets/sheet1.xml"),
        ])),
        ("xl/worksheets/sheet1.xml",
         ws("A1:B1", rows, extras_before=before, extras_after=after)),
    ]
    write_zip("05-unknown-subtrees.xlsx", members)


def gen_06_multisheet_1904():
    """06: 3 sheets, definedNames, date1904=1. Exercises multi-sheet order,
    the 1904 epoch flag, and workbook + sheet-scoped defined names."""
    sheets = [("Summary", 1, "rId1"), ("Data", 2, "rId2"), ("Notes", 3, "rId3")]
    names = [
        ("TaxRate", "0.2", None),
        ("DataRange", "Data!$A$1:$A$3", None),
        ("LocalName", "Notes!$B$2", 2),
    ]
    s1 = ws("A1:A1", '<row r="1"><c r="A1" t="n"><v>100</v></c></row>')
    s2 = ws("A1:A3",
            '<row r="1"><c r="A1" t="n"><v>1</v></c></row>'
            '<row r="2"><c r="A2" t="n"><v>2</v></c></row>'
            '<row r="3"><c r="A3" t="n"><v>3</v></c></row>')
    s3 = ws("B2:B2", '<row r="2"><c r="B2" t="inlineStr"><is><t>note</t></is></c></row>')
    members = [
        ("[Content_Types].xml", content_types([
            ("/xl/workbook.xml", CT_WORKBOOK),
            ("/xl/worksheets/sheet1.xml", CT_WORKSHEET),
            ("/xl/worksheets/sheet2.xml", CT_WORKSHEET),
            ("/xl/worksheets/sheet3.xml", CT_WORKSHEET),
        ])),
        ("_rels/.rels", root_rels()),
        ("xl/workbook.xml", workbook(sheets, date1904=True, defined_names=names)),
        ("xl/_rels/workbook.xml.rels", workbook_rels([
            ("rId1", RT_WORKSHEET, "worksheets/sheet1.xml"),
            ("rId2", RT_WORKSHEET, "worksheets/sheet2.xml"),
            ("rId3", RT_WORKSHEET, "worksheets/sheet3.xml"),
        ])),
        ("xl/worksheets/sheet1.xml", s1),
        ("xl/worksheets/sheet2.xml", s2),
        ("xl/worksheets/sheet3.xml", s3),
    ]
    write_zip("06-multisheet-1904.xlsx", members)


def main():
    gen_01_minimal()
    gen_02_formulas()
    gen_03_styles()
    gen_04_unknown_parts()
    gen_05_unknown_subtrees()
    gen_06_multisheet_1904()


if __name__ == "__main__":
    main()
