#!/usr/bin/env python3
# paged.sheet — classify the full recalc lane's disagreements.
#
# Usage:
#   PAGED_XLSX_CORPUS=~/paged/corpus PAGED_RECALC_REPORT=/tmp/r.tsv \
#     cargo test -p sheet-conformance --test xlsx_recalc_corpus -- --ignored --nocapture
#   python3 classify.py /tmp/r.tsv [~/paged/corpus/xlsx]
#
# Files each differing/unparsed cell under one cause, reading the workbook
# XML for what the engine's model does not keep (is the cell a shared-formula
# MEMBER? a legacy array formula? which defined names exist?).
import os, sys, zipfile, re, collections
import xml.etree.ElementTree as ET
M="{http://schemas.openxmlformats.org/spreadsheetml/2006/main}"
R="{http://schemas.openxmlformats.org/officeDocument/2006/relationships}"
root=(sys.argv[2] if len(sys.argv) > 2 else os.path.expanduser("~/paged/corpus/xlsx")).rstrip("/") + "/"
rep=sys.argv[1]
rows=[r for r in (l.rstrip("\n").split("\t") for l in open(rep)) if len(r)==8][1:]
cache={}
def info(src,f):
    k=(src,f)
    if k in cache: return cache[k]
    z=zipfile.ZipFile(root+src+"/"+f)
    wb=ET.fromstring(z.read("xl/workbook.xml"))
    rels=ET.fromstring(z.read("xl/_rels/workbook.xml.rels"))
    tgt={r.get("Id"):r.get("Target") for r in rels}
    out={}
    for s in wb.find(M+"sheets"):
        t=tgt[s.get(R+"id")].lstrip("/"); t=t if t.startswith("xl/") else "xl/"+t
        try: x=z.read(t).decode("utf8","replace")
        except KeyError: continue
        d={}
        for m in re.finditer(r'<c r="([A-Z]+\d+)"[^>]*>(.*?)</c>',x):
            fm=re.search(r'<f([^>]*)(/>|>(.*?)</f>)',m.group(2))
            if fm:
                a=fm.group(1)
                d[m.group(1)]=("shared-member" if 't="shared"' in a and not fm.group(3) else
                               "shared-master" if 't="shared"' in a else
                               "array" if 't="array"' in a else "plain")
        out[s.get("name")]=d
    names=wb.find(M+"definedNames")
    cache[k]=(out, [n.get("name") for n in names] if names is not None else [])
    return cache[k]
cls=collections.Counter(); ex={}
for src,f,sh,cell,klass,formula,cached,eng in rows:
    sheets,names=info(src,f)
    kind=sheets.get(sh,{}).get(cell,"?")
    if klass=="Unparsed":
        if re.search(r"_xludf\.|_XLUDF\.",formula,re.I): c="unparsed: _xludf (user-defined/addin fn)"
        elif re.search(r"\[\d+\]",formula): c="unparsed: external workbook ref"
        elif "_xlfn" in formula.lower(): c="unparsed: _xlfn prefixed"
        elif re.search(r"\w+\[",formula) or "[#" in formula: c="unparsed: structured ref"
        else: c="unparsed: unregistered function / unsupported syntax"
    elif kind=="shared-member": c="differs: shared-formula member"
    elif eng=="e:#NAME?":
        nm=[n for n in names if re.search(r"(?<![\w.])"+re.escape(n)+r"(?![\w(])",formula)]
        c="differs: engine #NAME? (defined name)" if nm else "differs: engine #NAME? (function)"
    elif kind=="array": c="differs: legacy array (CSE) formula"
    elif cached.startswith("n:") and eng.startswith("n:"): c="differs: number vs number"
    elif cached.startswith("s:") and eng.startswith("s:"): c="differs: text vs text"
    elif eng.startswith("e:"): c="differs: engine error "+eng
    else: c="differs: type ("+cached.split(":")[0]+" vs "+eng.split(":")[0]+")"
    cls[c]+=1; ex.setdefault(c,[]).append((f,sh,cell,formula,cached,eng))
for c,n in cls.most_common(): 
    print(f"{n:>6}  {c}")
    for e in ex[c][:2]: print("          ", " | ".join(x[:60] for x in e))
