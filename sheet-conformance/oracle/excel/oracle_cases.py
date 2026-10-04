# paged.sheet — Excel oracle lane: the shared case model.
#
# @copyright  Copyright (c) And The Next GmbH
# @license    AGPL-3.0-only OR Paged Media Enterprise License (PMEL)
#
# One module both `generate.py` and `read.py` import, so the workbook a case
# was written into and the sheet its result is read back from can never drift
# apart: the case order IS the sheet order, derived from the same walk.
#
# The TSV grammar mirrors `sheet-conformance/src/lib.rs::parse_corpus` and the
# setup grammar mirrors `tests/corpus_runner.rs::apply_setup` (which in turn
# mirrors `sheet_calc::literal_of`). If either changes, change this file in
# the same commit — the Rust lane re-checks every recorded formula against
# the live golden, so drift surfaces as a STALE row, never as a silent pass.

from __future__ import annotations

import os
import re
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]  # oracle/excel -> oracle -> sheet-conformance -> repo
FN_CORPUS = REPO / "corpus" / "fn-corpus"

# `coerce/` is a 3-column unit-fixture schema, not formula cases.
SKIP_FAMILIES = {"coerce"}

# The error literals `sheet_core::CellError::parse` accepts (anything else that
# starts with '#' is TEXT to the engine, so it is text here too).
ERRORS = {"#DIV/0!", "#VALUE!", "#REF!", "#NAME?", "#NUM!", "#N/A", "#NULL!",
          "#SPILL!"}

# Excel's "future function" prefixes (MS-XLSX 2.2.2). A function added after
# Excel 2007 is stored in the file with a prefix; written bare, Excel reads an
# unknown name and returns #NAME?. Only the names the corpus can reach need
# to be here — a missing one shows up as a recorded #NAME? that the engine
# does not produce, which the lane reports as `oracle-setup` (not a defect).
XLFN = {
    "AGGREGATE", "ARABIC", "BASE", "BITAND", "BITOR", "BITXOR", "BITLSHIFT",
    "BITRSHIFT", "CEILING.MATH", "CEILING.PRECISE", "CHISQ.DIST", "CHOOSECOLS",
    "CHOOSEROWS", "COMBINA", "CONCAT", "COT", "COTH", "COVARIANCE.P",
    "COVARIANCE.S", "CSC", "CSCH", "DAYS", "DECIMAL", "DROP", "EXPAND",
    "FLOOR.MATH", "FLOOR.PRECISE", "FORECAST.LINEAR", "FORMULATEXT", "GAMMA",
    "HSTACK", "IFNA", "IFS", "ISFORMULA", "ISOWEEKNUM", "LAMBDA", "LET",
    "MAP", "MAXIFS", "MINIFS", "MODE.MULT", "MODE.SNGL", "NETWORKDAYS.INTL",
    "NORM.DIST", "NORM.INV", "NORM.S.DIST", "NORM.S.INV", "NUMBERVALUE",
    "PERCENTILE.EXC", "PERCENTILE.INC", "PERCENTRANK.EXC", "PERCENTRANK.INC", "QUARTILE.EXC", "QUARTILE.INC",
    "RANDARRAY", "RANK.AVG", "RANK.EQ", "REDUCE", "SCAN", "SEC", "SECH",
    "SEQUENCE", "SHEET", "SHEETS", "SORTBY", "STDEV.P", "STDEV.S", "SWITCH",
    "TAKE", "TEXTAFTER", "TEXTBEFORE", "TEXTJOIN", "TEXTSPLIT", "TOCOL",
    "TOROW", "UNICHAR", "UNICODE", "UNIQUE", "VALUETOTEXT", "ARRAYTOTEXT",
    "VAR.P", "VAR.S", "VSTACK", "WORKDAY.INTL", "WRAPCOLS", "WRAPROWS",
    "XLOOKUP", "XMATCH", "XOR", "ERF.PRECISE", "ERFC.PRECISE", "ACOT",
    "ACOTH", "ISOMITTED", "BYROW", "BYCOL", "MAKEARRAY",
    "GAMMALN.PRECISE", "PHI", "GAUSS", "SKEW.P", "PDURATION", "RRI",
    "BINOM.DIST", "EXPON.DIST", "T.DIST", "T.INV", "F.DIST", "LOGNORM.DIST",
    "POISSON.DIST", "WEIBULL.DIST", "GAMMA.DIST", "BETA.DIST", "CONFIDENCE.NORM",
    "CONFIDENCE.T", "STDEVA", "ENCODEURL", "FILTERXML", "WEBSERVICE",
}
# Worksheet-only dynamic-array functions carry a second namespace.
XLWS = {"FILTER", "SORT"}


@dataclass
class Case:
    family: str
    file: str            # e.g. "sum.golden.tsv"
    id: str
    formula: str         # as in the golden (leading '=')
    setup: list[tuple[str, str]]
    expected: str
    host: str = "Z99"
    sheet: str = ""      # assigned per workbook: c1, c2, ...

    @property
    def key(self) -> str:
        return f"{self.file}\t{self.id}"


def normalize_separators(formula: str) -> str:
    """Top-level ';' -> ',' (runner.rs::normalize_separators); ';' inside a
    string or an array literal {1,2;3,4} (the row separator) is kept."""
    out, in_str, i, braces = [], False, 0, 0
    while i < len(formula):
        ch = formula[i]
        if ch == '"':
            if in_str and i + 1 < len(formula) and formula[i + 1] == '"':
                out.append('""')
                i += 2
                continue
            in_str = not in_str
            out.append(ch)
        elif ch == "{" and not in_str:
            braces += 1
            out.append(ch)
        elif ch == "}" and not in_str:
            braces = max(0, braces - 1)
            out.append(ch)
        elif ch == ";" and not in_str and braces == 0:
            out.append(",")
        else:
            out.append(ch)
        i += 1
    return "".join(out)


_FN = re.compile(r'(?<![A-Za-z0-9_.])([A-Z][A-Z0-9]*(?:\.[A-Z0-9]+)*)\(')


def _call_args(text: str, open_paren: int) -> list[str]:
    """Top-level argument texts of the call whose '(' is at `open_paren`."""
    args, depth, cur, in_str, i = [], 0, [], False, open_paren + 1
    while i < len(text):
        ch = text[i]
        if ch == '"':
            in_str = not in_str
        elif not in_str:
            if ch in "({":
                depth += 1
            elif ch in ")}":
                if depth == 0:
                    args.append("".join(cur))
                    return args
                depth -= 1
            elif ch == "," and depth == 0:
                args.append("".join(cur))
                cur = []
                i += 1
                continue
        cur.append(ch)
        i += 1
    return args


def lambda_params(body: str) -> set[str]:
    """Names bound by LET / LAMBDA anywhere in the formula (upper-cased).

    LAMBDA(p1, ..., body): every argument but the last. LET(n1, v1, ..., body):
    the odd-position arguments. In the file format each USE of such a name is
    written `_xlpm.<name>`; written bare, Excel reads an undefined name.
    """
    names: set[str] = set()
    for m in re.finditer(r'(?<![A-Za-z0-9_.])(LAMBDA|LET)\(', body):
        args = [a.strip() for a in _call_args(body, m.end() - 1)]
        if m.group(1) == "LAMBDA":
            names.update(a.upper() for a in args[:-1])
        else:
            names.update(a.upper() for a in args[:-1:2])
    return {n for n in names if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.]*", n)}


_IDENT = re.compile(r'(?<![A-Za-z0-9_.])([A-Za-z_][A-Za-z0-9_.]*)(?![A-Za-z0-9_.])')


def to_file_formula(formula: str) -> str:
    """Golden formula -> xlsx `<f>` text: no '=', en-US separators, prefixes.

    String literals are copied untouched (a function-looking word inside a
    string must not gain a prefix).
    """
    body = normalize_separators(formula)
    if body.startswith("="):
        body = body[1:]
    parts = re.split(r'("(?:[^"]|"")*")', body)
    for i in range(0, len(parts), 2):
        def sub(m: re.Match) -> str:
            name = m.group(1)
            if name in XLWS:
                return f"_xlfn._xlws.{name}("
            if name in XLFN:
                return f"_xlfn.{name}("
            return m.group(0)
        parts[i] = _FN.sub(sub, parts[i])
    params = lambda_params(body)
    if params:
        for i in range(0, len(parts), 2):
            parts[i] = _IDENT.sub(
                lambda m: f"_xlpm.{m.group(1)}" if m.group(1).upper() in params else m.group(0),
                parts[i])
    return "".join(parts)


def parse_setup(col: str) -> list[tuple[str, str]]:
    if not col:
        return []
    out = []
    for seed in col.split(";"):
        if not seed:
            continue
        if "=" in seed:
            a, v = seed.split("=", 1)
        else:
            a, v = seed, ""
        out.append((a, v))
    return out


FORMAT_CORPUS = REPO / "corpus" / "format-corpus"
FORMAT_FAMILY = "format"


def format_formula(code: str) -> str:
    """The probe for a number-format case: Excel's TEXT() over the seeded
    value (tests/excel_oracle.rs builds the identical string)."""
    return '=TEXT(A1,"' + code.replace('"', '""') + '")'


def format_seed(value: str) -> str:
    if value.startswith("bool:"):
        return "bool:" + value[5:].upper()
    return value


def load_format_family() -> list[Case]:
    """corpus/format-corpus rows (id, code, value, expected) as TEXT() cases."""
    cases: list[Case] = []
    # locale-*.golden.tsv render under other locales; this oracle is en-US.
    for path in sorted(p for p in FORMAT_CORPUS.glob("*.golden.tsv")
                       if not p.name.startswith("locale-")):
        for raw in path.read_text(encoding="utf-8").splitlines():
            line = raw.rstrip("\r\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            cols = line.split("\t")
            if len(cols) == 3:   # an empty expected (e.g. ';;;') loses its tab
                cols.append("")
            if len(cols) != 4:
                raise SystemExit(f"{path}: {len(cols)} columns in {line!r}")
            cid, code, value, expected = cols
            cases.append(Case(FORMAT_FAMILY, path.name, cid, format_formula(code),
                              [("A1", format_seed(value))], expected))
    for n, c in enumerate(cases, 1):
        c.sheet = f"c{n}"
    return cases


def load_family(family: str) -> list[Case]:
    if family == FORMAT_FAMILY:
        return load_format_family()
    cases: list[Case] = []
    for path in sorted((FN_CORPUS / family).glob("*.golden.tsv")):
        for raw in path.read_text(encoding="utf-8").splitlines():
            line = raw.rstrip("\r\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            cols = line.split("\t")
            if len(cols) != 4:
                raise SystemExit(f"{path}: {len(cols)} columns in {line!r}")
            c = Case(family, path.name, cols[0], cols[1], parse_setup(cols[2]), cols[3])
            for addr, _ in c.setup:
                if addr.startswith("@"):
                    c.host = addr[1:].upper()
            cases.append(c)
    for n, c in enumerate(cases, 1):
        c.sheet = f"c{n}"
    return cases


def families() -> list[str]:
    return sorted(d.name for d in FN_CORPUS.iterdir()
                  if d.is_dir() and d.name not in SKIP_FAMILIES) + [FORMAT_FAMILY]


_NUM = re.compile(r"^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$")


def typed_setup(raw: str):
    """Setup VALUE -> (kind, payload); mirrors corpus_runner + literal_of.

    kind in {blank, text, bool, error, number, formula}.
    """
    if raw == "empty" or raw == "":
        return ("blank", None)
    if raw.startswith("text:"):
        return ("text", raw[5:])
    if raw.startswith("bool:"):
        return ("bool", raw[5:].strip().upper() == "TRUE")
    if raw.startswith("="):
        return ("formula", raw)
    if raw.upper() in ("TRUE", "FALSE"):
        return ("bool", raw.upper() == "TRUE")
    if raw in ERRORS:
        return ("error", raw)
    if raw == raw.strip() and "_" not in raw and _NUM.match(raw):
        v = float(raw)
        if v == v and abs(v) != float("inf"):
            return ("number", raw)
    return ("text", raw)


def a1(addr: str) -> tuple[int, int]:
    """'B3' -> (row 3, col 2), 1-based."""
    m = re.match(r"^([A-Za-z]+)(\d+)$", addr.strip())
    if not m:
        raise ValueError(addr)
    col = 0
    for ch in m.group(1).upper():
        col = col * 26 + (ord(ch) - 64)
    return int(m.group(2)), col


# The locale guard. Excel computes in its UI language and the system region:
# under de-AT, CONCAT(TRUE) is "WAHR", VALUE("3.14") is the date 14 March and
# TEXT(x,"#,##0.00") swaps the separators — every one of those reads as an
# engine defect while being a harness artifact. Every workbook carries this
# probe as its LAST sheet; read.py refuses to record a workbook whose probe
# does not come back in en-US.
LOCALE_PROBE_SHEET = "locale_probe"
LOCALE_PROBE_FORMULA = '=CONCAT(TRUE,"|",1.5,"|",TEXT(1234.5,"#,##0.00"),"|",ADDRESS(2,3,1,FALSE))'
LOCALE_PROBE_EXPECTED = "TRUE|1.5|1,234.50|R2C3"


def rejected_keys() -> set[str]:
    """`file<TAB>id` of every case Excel refuses to enter (excel-rejects.tsv)."""
    out = set()
    for line in (HERE / "excel-rejects.tsv").read_text(encoding="utf-8").splitlines():
        if line.strip() and not line.startswith("#"):
            f, i = line.split("\t")[:2]
            out.add(f"{f}\t{i}")
    return out


def unrepresentable(case: Case) -> str | None:
    """Why a case's SETUP cannot be written into a workbook, or None.

    `#SPILL!` (and `#CALC!`) are results, not constants: Excel's file format
    has no literal for them (a `t="e"` cell holding one makes Excel refuse
    the whole workbook), and a user cannot type one either.
    """
    for _, raw in case.setup:
        if raw in ("#SPILL!", "#CALC!"):
            return f"setup seeds the literal {raw}, which Excel cannot store"
    return None


def oracle_dir() -> Path:
    return HERE


def build_dir() -> Path:
    d = Path(os.environ.get("PAGED_ORACLE_BUILD", HERE / "build"))
    d.mkdir(parents=True, exist_ok=True)
    return d
