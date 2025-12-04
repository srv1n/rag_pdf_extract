#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
bootstrap_benchmark.py

Downloads a compact PDF extraction benchmark from DocLayNet, PubLayNet (+ PMC PDFs),
and PMC OA, and emits canonical JSON references suitable for evaluating a PDF parser.

Dependencies: requests, tqdm, pyyaml, lxml, beautifulsoup4, huggingface_hub, datasets
"""
from __future__ import annotations
import os, re, io, sys, json, gzip, tarfile, unicodedata, argparse, hashlib, textwrap, time
from pathlib import Path
from typing import Dict, List, Any, Optional, Tuple

def _require(pkg: str, pip_name: Optional[str] = None):
    try:
        __import__(pkg)
    except Exception as e:
        name = pip_name or pkg
        print(f"ERROR: Python package '{name}' not installed.\n"
              f"Install with: pip install {name}")
        raise

_require('requests')
_require('tqdm')
_require('yaml', 'pyyaml')
_require('lxml')
_require('bs4', 'beautifulsoup4')
_require('huggingface_hub')
_require('datasets')

import requests
from tqdm import tqdm
import yaml
from lxml import etree
from huggingface_hub import HfFileSystem, hf_hub_download
from datasets import load_dataset

# ------------------------------ Canonical helpers --------------------------------

def nfkc_ws(s: str) -> str:
    if s is None:
        return ""
    return " ".join(unicodedata.normalize("NFKC", s).split())

def make_block(role: str, text: str = "", bbox: Optional[Dict[str, float]] = None, level: Optional[int]=None) -> Dict[str, Any]:
    block = {"role": role, "text": nfkc_ws(text)}
    if bbox is not None:
        block["bbox"] = {
            "x": max(0.0, float(bbox["x"])),
            "y": max(0.0, float(bbox["y"])),
            "width": max(0.0, float(bbox["width"])),
            "height": max(0.0, float(bbox["height"])),
        }
    if level is not None and role == "heading":
        block["level"] = int(level)
    return block

def canonical_doc() -> Dict[str, Any]:
    return {"pages": []}

# ------------------------------ IO helpers --------------------------------

def ensure_dir(p: Path) -> None:
    p.mkdir(parents=True, exist_ok=True)

def download_to(url: str, out: Path, desc: Optional[str]=None) -> Path:
    out.parent.mkdir(parents=True, exist_ok=True)
    with requests.get(url, stream=True, timeout=60) as r:
        r.raise_for_status()
        total = int(r.headers.get("content-length", 0))
        with open(out, "wb") as f, tqdm(total=total, unit="B", unit_scale=True, desc=desc or out.name) as pbar:
            for chunk in r.iter_content(chunk_size=1<<15):
                if chunk:
                    f.write(chunk)
                    pbar.update(len(chunk))
    return out

def write_json(p: Path, obj: Any) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    with open(p, "w", encoding="utf-8") as f:
        json.dump(obj, f, ensure_ascii=False, indent=2)

def _to_jsonable(o: Any):
    if isinstance(o, (str, int, float, bool)) or o is None:
        return o
    if isinstance(o, dict):
        return {str(k): _to_jsonable(v) for k, v in o.items()}
    if isinstance(o, (list, tuple)):
        return [_to_jsonable(x) for x in o]
    # Fallback to string for non-serializable objects (e.g., PIL image)
    try:
        return str(o)
    except Exception:
        return "<non-serializable>"

# ------------------------------ PMC OA web service -------------------------
OA_FCGI = "https://pmc.ncbi.nlm.nih.gov/utils/oa/oa.fcgi"

def pmc_oa_links(pmcid: str) -> Dict[str, str]:
    """Return dict of useful links for a PMCID via OA Web Service: keys 'pdf', 'tgz', 'xml' when present."""
    r = requests.get(OA_FCGI, params={"id": pmcid}, timeout=30)
    r.raise_for_status()
    tree = etree.fromstring(r.content)
    out = {}
    for link in tree.xpath(".//link"):
        href = link.get("href", "")
        fmt  = link.get("format", "").lower()
        if not href:
            continue
        if fmt in ("pdf", "tgz", "xml"):
            out[fmt] = href
    lic = tree.xpath("string(//license/@href)") or tree.xpath("string(//license)")
    if lic:
        out["license"] = lic
    return out

# Europe PMC helper to discover OA PMCIDs
EPMC_SEARCH = "https://www.ebi.ac.uk/europepmc/webservices/rest/search"

def epmc_find_oa_pmcids(count: int = 10) -> List[str]:
    """Return up to `count` PMCIDs that are Open Access in PMC (source=PMC)."""
    pmcids: List[str] = []
    cursor = "*"
    tries = 0
    while len(pmcids) < count and tries < 10:
        params = {
            "query": "(OPEN_ACCESS:y) AND (SRC:PMC)",
            "format": "json",
            "pageSize": str(min(25, count * 2)),
            "cursorMark": cursor,
            "resultType": "core",
            "sort": "cited desc"
        }
        r = requests.get(EPMC_SEARCH, params=params, timeout=30)
        r.raise_for_status()
        j = r.json()
        results = (j.get("resultList") or {}).get("result") or []
        for res in results:
            pmcid = res.get("pmcid")
            if isinstance(pmcid, str) and pmcid.startswith("PMC"):
                pmcids.append(pmcid)
                if len(pmcids) >= count:
                    break
        cursor = j.get("nextCursorMark") or None
        if not cursor:
            break
        tries += 1
    # Dedup and cap
    seen = set()
    uniq: List[str] = []
    for p in pmcids:
        if p not in seen:
            seen.add(p)
            uniq.append(p)
            if len(uniq) >= count:
                break
    return uniq

# ------------------------------ DocLayNet ----------------------------------
DOC_LAYNET_REPO = "ds4sd/DocLayNet"

DLN_ROLE_MAP = {
    "Title": "heading",
    "Section-header": "heading",
    "Text": "paragraph",
    "List-item": "list",
    "Table": "table",
    "Caption": "caption",
}

def discover_doclaynet_paths(fs: HfFileSystem) -> Dict[str, List[str]]:
    root = "datasets/" + DOC_LAYNET_REPO
    files = fs.glob(root + "/**")
    # HfFileSystem.glob usually returns a list of path strings
    paths = []
    for f in files:
        if isinstance(f, str):
            paths.append(f)
        elif isinstance(f, dict) and f.get("type") == "file" and "name" in f:
            paths.append(f["name"])
    coco = [p for p in paths if p.lower().endswith(".json") and ("coco" in p.lower() or "/annotations" in p.lower())]
    pdfs = [p for p in paths if p.lower().endswith(".pdf") and ("extra" in p.lower() or "single" in p.lower())]
    textc = [p for p in paths if p.lower().endswith(".json") and ("text" in p.lower() and "cell" in p.lower())]
    return {"coco_jsons": sorted(coco), "single_page_pdfs": sorted(pdfs), "textcells_jsons": sorted(textc)}

def load_coco_json_from_hf(path_in_repo: str) -> Dict[str, Any]:
    rel = path_in_repo.replace(f"datasets/{DOC_LAYNET_REPO}/", "")
    tmp = hf_hub_download(repo_id=DOC_LAYNET_REPO, repo_type="dataset", filename=rel)
    with open(tmp, "r", encoding="utf-8") as f:
        return json.load(f)

def pick_doclaynet_pages(coco: Dict[str, Any], k: int) -> List[Dict[str, Any]]:
    images = coco.get("images", [])
    by_cat: Dict[str, List[dict]] = {}
    for im in images:
        cat = im.get("doc_category") or "unknown"
        by_cat.setdefault(cat, []).append(im)
    out = []
    cats = sorted(by_cat.keys())
    i = 0
    while len(out) < min(k, len(images)) and cats:
        cat = cats[i % len(cats)]
        if by_cat[cat]:
            out.append(by_cat[cat].pop(0))
        i += 1
        if all(len(v) == 0 for v in by_cat.values()):
            break
    if len(out) < k:
        need = k - len(out)
        extras = [im for im in images if im not in out][:need]
        out.extend(extras)
    return out

def build_canonical_from_doclaynet(coco: Dict[str, Any], image_record: Dict[str, Any]) -> Dict[str, Any]:
    img_id = image_record["id"]
    id2cat = {c["id"]: c["name"] for c in coco.get("categories", [])}
    canonical = canonical_doc()
    page = {"page_num": int(image_record.get("page_no") or 1), "blocks": []}
    for ann in coco.get("annotations", []):
        if ann.get("image_id") != img_id:
            continue
        cat_name = id2cat.get(ann.get("category_id"))
        role = DLN_ROLE_MAP.get(cat_name)
        if not role:
            continue
        x, y, bw, bh = ann["bbox"]
        page["blocks"].append(make_block(role=role, text="", bbox={"x": x, "y": y, "width": bw, "height": bh}))
    canonical["pages"].append(page)
    return canonical

# ------------------------------ PubLayNet ----------------------------------
PLN_DATASET_ID = "jordanparker6/publaynet"

PLN_ROLE_MAP = {
    "title": "heading",
    "text": "paragraph",
    "list": "list",
    "table": "table",
    "figure": "caption",
}

PMCID_RE = re.compile(r"(PMC\d+)")

def build_canonical_from_publaynet_row(row: Dict[str, Any], page_num: int = 1) -> Dict[str, Any]:
    canonical = canonical_doc()
    page = {"page_num": page_num, "blocks": []}
    anns = row.get("annotations", [])
    for a in anns:
        name = None
        if "category" in a and isinstance(a["category"], dict) and "name" in a["category"]:
            name = str(a["category"]["name"]).lower()
        elif "name" in a:
            name = str(a["name"]).lower()
        if not name:
            continue
        role = PLN_ROLE_MAP.get(name)
        if not role:
            continue
        x, y, bw, bh = a["bbox"]
        page["blocks"].append(make_block(role=role, text="", bbox={"x": x, "y": y, "width": bw, "height": bh}))
    canonical["pages"].append(page)
    return canonical

def row_pmcid_hint(row: Dict[str, Any]) -> Optional[str]:
    for key in ("file_name", "image", "id", "image_id"):
        v = row.get(key)
        if isinstance(v, dict) and "path" in v:
            m = PMCID_RE.search(v["path"])
            if m: return m.group(1)
        if isinstance(v, str):
            m = PMCID_RE.search(v)
            if m: return m.group(1)
    meta = row.get("meta") or {}
    if isinstance(meta, dict):
        v = meta.get("pmcid") or meta.get("PMCID")
        if isinstance(v, str) and PMCID_RE.fullmatch(v):
            return v
    return None

# ------------------------------ PMC JATS → canonical -----------------------

def pmc_jats_to_canonical(xml_bytes: bytes) -> Dict[str, Any]:
    doc = canonical_doc()
    page = {"page_num": 1, "blocks": []}
    if not xml_bytes:
        doc["pages"].append(page)
        return doc
    tree = etree.fromstring(xml_bytes)
    ns = {}
    def walk_sec(elem, level=1):
        title = elem.find("title", namespaces=ns)
        if title is not None and title.text:
            page["blocks"].append(make_block(role="heading", text=title.text, level=level))
        for p in elem.findall("p", namespaces=ns):
            txt = "".join(p.itertext())
            if txt.strip():
                page["blocks"].append(make_block(role="paragraph", text=txt))
        for sub in elem.findall("sec", namespaces=ns):
            walk_sec(sub, level=min(level+1, 6))
    for sec in tree.findall(".//sec", namespaces=ns):
        parent = sec.getparent()
        if parent is not None and parent.tag.endswith("sec"):
            continue
        walk_sec(sec, level=1)
    doc["pages"].append(page)
    return doc

# ------------------------------ Orchestration ------------------------------

def write_dataset_card(report_dir: Path):
    ensure_dir(report_dir)
    p = report_dir / "dataset_card.md"
    p.write_text(textwrap.dedent(f"""
    # Mini PDF-Extraction Benchmark (DocLayNet + PubLayNet + PMC OA)

    - **DocLayNet**: human-annotated COCO boxes; extra single-page PDFs and text-cell JSON; license CDLA‑Permissive‑1.0.
    - **PubLayNet**: COCO boxes on PMC‑OA papers; page‑PDFs released; derived from PMC OA.
    - **PMC OA**: PDFs + JATS via the OA Web Service; licenses are CC variants.

    This subset is intended for reproducible parser evaluation in research settings.
    """ ).strip()+"\n", encoding="utf-8")

def write_index_html(report_dir: Path, entries: List[Dict[str, Any]]):
    ensure_dir(report_dir)
    p = report_dir / "index.html"
    rows = []
    for e in entries:
        rows.append(f"<tr><td>{e['doc_id']}</td><td>{e['source']}</td><td>{'✔' if e['has_text'] else ''}</td><td>{'✔' if e['has_headings'] else ''}</td><td>{'✔' if e['has_bboxes'] else ''}</td></tr>")
    html = f"""<!doctype html><html><head><meta charset='utf-8'><title>Benchmark Overview</title>
    <style>body{{font-family:sans-serif}}table{{border-collapse:collapse}}td,th{{border:1px solid #ddd;padding:6px}}</style>
    </head><body>
    <h1>Benchmark Overview</h1>
    <table><thead><tr><th>doc_id</th><th>source</th><th>text</th><th>headings</th><th>bboxes</th></tr></thead>
    <tbody>{''.join(rows)}</tbody></table></body></html>"""
    p.write_text(html, encoding="utf-8")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="benchmarks", help="Output folder root")
    ap.add_argument("--doclaynet", type=int, default=6, help="How many DocLayNet pages to fetch (0 to skip)")
    ap.add_argument("--publaynet", type=int, default=6, help="How many PubLayNet pages to fetch (0 to skip)")
    ap.add_argument("--pmcids", type=str, default="", help="Comma-separated PMCIDs to fetch (e.g., PMC8542361,PMC10139909)")
    ap.add_argument("--pmc-auto", type=int, default=0, help="Auto-discover this many OA PMCIDs (via Europe PMC)")
    ap.add_argument("--allow-noncommercial", type=int, default=1, help="Allow OA items with non-commercial licenses (1=yes default, 0=no)")
    args = ap.parse_args()

    root = Path(args.out)
    pdf_dir = root / "pdfs"
    refs_dir = root / "refs"
    report_dir = root / "reports"
    ensure_dir(pdf_dir); ensure_dir(refs_dir); ensure_dir(report_dir)

    manifest = {"docs": []}
    manifest_rows = []

    # ---------- DocLayNet ----------
    if args.doclaynet > 0:
        print("== DocLayNet: discovering files on Hugging Face ...")
        fs = HfFileSystem()
        paths = discover_doclaynet_paths(fs)
        coco_path = None
        for pref in ("val", "test", "train"):
            cands = [p for p in paths["coco_jsons"] if re.search(rf"{pref}.*\.json$", p, re.I)]
            if cands:
                coco_path = cands[0]; break
        if not coco_path and paths["coco_jsons"]:
            coco_path = paths["coco_jsons"][0]
        if not coco_path:
            print("WARNING: Could not find DocLayNet COCO JSON; skipping DocLayNet.")
        else:
            coco = load_coco_json_from_hf(coco_path)
            picked = pick_doclaynet_pages(coco, args.doclaynet)
            for im in picked:
                doc_name = im.get("doc_name") or ""
                page_no  = im.get("page_no") or im.get("page_index") or 1
                image_id = im["id"]
                pdf_hf_path = None
                for p in paths["single_page_pdfs"]:
                    if doc_name and doc_name in os.path.basename(p):
                        if str(page_no) in p or f"_{int(page_no):02d}" in p or True:
                            pdf_hf_path = p; break
                if pdf_hf_path:
                    rel = pdf_hf_path.replace(f"datasets/{DOC_LAYNET_REPO}/", "")
                    local_pdf = hf_hub_download(repo_id=DOC_LAYNET_REPO, repo_type="dataset", filename=rel)
                    name_hash = hashlib.sha256((doc_name+str(page_no)).encode()).hexdigest()[:10]
                    out_pdf = pdf_dir / f"dl_doclaynet_{name_hash}.pdf"
                    Path(local_pdf).replace(out_pdf)
                else:
                    out_pdf = pdf_dir / f"dl_doclaynet_missing_{image_id}.pdf"
                    out_pdf.write_bytes(b"")
                canonical = build_canonical_from_doclaynet(coco, im)
                doc_id = f"dl_doclaynet_{image_id}"
                doc_dir = refs_dir / doc_id
                ensure_dir(doc_dir)
                ann_slice = {
                    "images": [im],
                    "annotations": [a for a in coco.get("annotations", []) if a.get("image_id")==image_id],
                    "categories": coco.get("categories", [])
                }
                write_json(doc_dir / "vendor-DocLayNet.coco.json", ann_slice)
                write_json(doc_dir / "canonical.json", canonical)
                manifest["docs"].append({
                    "doc_id": doc_id,
                    "source": "DocLayNet",
                    "license": "CDLA-Permissive-1.0",
                    "pdf_path": str(out_pdf.as_posix()),
                    "reference": {
                        "primary_vendor": "DocLayNet",
                        "has_text": False,
                        "has_headings": True,
                        "has_bboxes": True,
                        "notes": f"{doc_name}, page {page_no}"
                    }
                })

    # ---------- PubLayNet ----------
    if args.publaynet > 0:
        print("== PubLayNet: loading HF dataset and sampling pages ...")
        ds = None
        streaming = False
        try:
            # Prefer streaming to avoid large local downloads
            ds = load_dataset(PLN_DATASET_ID, split="validation", streaming=True)
            streaming = True
        except Exception:
            try:
                ds = load_dataset(PLN_DATASET_ID, split="train", streaming=True)
                streaming = True
            except Exception:
                # Fallback (may be huge) — will likely fail on low disk
                try:
                    ds = load_dataset(PLN_DATASET_ID, split="validation")
                except Exception:
                    ds = load_dataset(PLN_DATASET_ID, split="train")
        # Iterate k rows
        it = iter(ds) if streaming else range(min(args.publaynet, len(ds)))
        count = 0
        for i_idx in it:
            if count >= args.publaynet:
                break
            row = i_idx if streaming else ds[i_idx]
            pmcid = row_pmcid_hint(row)
            out_pdf = None
            if pmcid:
                links = pmc_oa_links(pmcid)
                pdf_url = links.get("pdf")
                if (not pdf_url) and links.get("tgz"):
                    tgz_path = download_to(links["tgz"], pdf_dir / f"{pmcid}.tgz", desc=f"{pmcid}.tgz")
                    import tarfile
                    with tarfile.open(tgz_path, "r:gz") as tar:
                        pdf_members = [m for m in tar.getmembers() if m.name.lower().endswith(".pdf")]
                        if pdf_members:
                            m = pdf_members[0]
                            f = tar.extractfile(m)
                            data = f.read()
                            out_pdf = pdf_dir / f"pln_{pmcid}.pdf"
                            out_pdf.write_bytes(data)
                elif pdf_url:
                    out_pdf = download_to(pdf_url, pdf_dir / f"pln_{pmcid}.pdf", desc=f"{pmcid}.pdf")
            canonical = build_canonical_from_publaynet_row(row, page_num=1)
            doc_id = f"pln_{pmcid or 'unknown'}_{count}"
            doc_dir = refs_dir / doc_id
            ensure_dir(doc_dir)
            try:
                vendor_row = dict(row)
                if "image" in vendor_row:
                    vendor_row.pop("image")
                write_json(doc_dir / "vendor-PubLayNet.row.json", _to_jsonable(vendor_row))
            except Exception:
                # Be robust: skip vendor row dump if types are unexpected
                write_json(doc_dir / "vendor-PubLayNet.row.json", {"note": "vendor row omitted due to serialization issues"})
            write_json(doc_dir / "canonical.json", canonical)
            manifest["docs"].append({
                "doc_id": doc_id,
                "source": "PubLayNet",
                "license": "PMC OA license per article; PubLayNet annotations research-use",
                "pdf_path": str(out_pdf.as_posix()) if out_pdf else "",
                "reference": {
                    "primary_vendor": "PubLayNet",
                    "has_text": False,
                    "has_headings": True,
                    "has_bboxes": True,
                    "notes": f"PMCID={pmcid or 'unknown'}; bbox-only"
                }
            })
            count += 1

    # ---------- PMC OA (JATS + PDF) ----------
    auto_pmcs: List[str] = []
    if args.pmc_auto and args.pmc_auto > 0:
        print(f"== PMC OA: discovering {args.pmc_auto} OA PMCIDs via Europe PMC ...")
        try:
            auto_pmcs = epmc_find_oa_pmcids(args.pmc_auto)
            print(f"Discovered {len(auto_pmcs)} PMCIDs: {', '.join(auto_pmcs)}")
        except Exception as e:
            print(f"WARNING: Europe PMC discovery failed: {e}")

    pmc_list = []
    if args.pmcids:
        print("== PMC OA: downloading PDFs + JATS and converting XML to canonical ...")
        pmc_list.extend([x.strip() for x in args.pmcids.split(",") if x.strip()])
    if auto_pmcs:
        # Merge in discovered pmcids, keep unique
        for p in auto_pmcs:
            if p not in pmc_list:
                pmc_list.append(p)
        if not args.pmcids:
            print("== PMC OA: downloading PDFs + JATS and converting XML to canonical ...")

    if pmc_list:
        kept = 0
        for pmcid in pmc_list:
            try:
                links = pmc_oa_links(pmcid)
            except Exception as e:
                print(f"Skipping {pmcid}: OA Web Service error: {e}")
                continue
            lic = links.get("license", "")
            if not args.allow_noncommercial and ("nc" in lic.lower()):
                print(f"Skipping {pmcid} due to non-commercial license: {lic}")
                continue
            pdf_url = links.get("pdf")
            if pdf_url:
                out_pdf = download_to(pdf_url, pdf_dir / f"pmc_{pmcid}.pdf", desc=f"{pmcid}.pdf")
            else:
                out_pdf = pdf_dir / f"pmc_{pmcid}_missing.pdf"
                out_pdf.write_bytes(b"")
            xml_bytes = b""
            if links.get("xml"):
                xml_bytes = requests.get(links["xml"], timeout=30).content
            elif links.get("tgz"):
                tgz_path = download_to(links["tgz"], pdf_dir / f"{pmcid}.tgz", desc=f"{pmcid}.tgz")
                import tarfile
                with tarfile.open(tgz_path, "r:gz") as tar:
                    xml_members = [m for m in tar.getmembers() if m.name.lower().endswith((".nxml", ".xml"))]
                    if xml_members:
                        f = tar.extractfile(xml_members[0]); xml_bytes = f.read()
            canonical = pmc_jats_to_canonical(xml_bytes) if xml_bytes else canonical_doc()
            doc_id = f"pmc_{pmcid}"
            doc_dir = refs_dir / doc_id
            ensure_dir(doc_dir)
            if xml_bytes:
                (doc_dir / "vendor-PMC.jats.xml").write_bytes(xml_bytes)
            write_json(doc_dir / "canonical.json", canonical)
            manifest["docs"].append({
                "doc_id": doc_id,
                "source": "PMC_JATS",
                "license": lic or "See OA Web Service record",
                "pdf_path": str(out_pdf.as_posix()),
                "reference": {
                    "primary_vendor": "PMC_JATS",
                    "has_text": True,
                    "has_headings": True,
                    "has_bboxes": False,
                    "notes": "Text + headings from JATS; no bboxes"
                }
            })
            kept += 1

    # ---------- write manifest & tiny summary ----------
    (root / "dataset.yml").write_text(yaml.safe_dump(manifest, sort_keys=False), encoding="utf-8")
    write_dataset_card(report_dir)
    entries = [
        {"doc_id": m["doc_id"], "source": m["source"], "has_text": m["reference"]["has_text"],
         "has_headings": m["reference"]["has_headings"], "has_bboxes": m["reference"]["has_bboxes"]}
        for m in manifest["docs"]
    ]
    write_index_html(report_dir, entries)
    print(f"\nDone. Wrote manifest with {len(manifest['docs'])} entries to {root/'dataset.yml'}")

if __name__ == "__main__":
    main()
