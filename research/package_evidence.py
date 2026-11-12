"""Normalize Windows text logs, validate documentation links, and package shareable evidence.

The archive contains research/ and should be extracted over the recorded upstream clone.
Large generated outputs, fixtures, virtual environments, and build products are excluded.
"""
import hashlib, json, pathlib, re, subprocess, zipfile
ROOT=pathlib.Path(__file__).resolve().parents[1]
RESEARCH=ROOT/'research'
skip={'.venv','target','fixtures','__pycache__','video-output','image-output','fit-output','manual-output'}
def included(p):
    return p.is_file() and not any(part in skip for part in p.relative_to(RESEARCH).parts) and p.name!='artifact-manifest.json'

for p in RESEARCH.rglob('*'):
    if included(p) and p.suffix in {'.log','.txt'}:
        data=p.read_bytes()
        if data.startswith((b'\xff\xfe',b'\xfe\xff')):
            p.write_text(data.decode('utf-16'),encoding='utf8')

for doc in RESEARCH.glob('*.md'):
    for target in re.findall(r'\]\(([^)]+)\)',doc.read_text(encoding='utf8')):
        if target.startswith(('https://','http://','#')):continue
        assert (doc.parent/target.split('#')[0]).exists(), (doc,target)

files=sorted(p for p in RESEARCH.rglob('*') if included(p))
manifest={'upstream_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
          'files':[{'path':str(p.relative_to(ROOT)).replace('\\','/'),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in files]}
for name,p in [('upstream_binary',ROOT/'target/release/cascii.exe'),('prototype_binary',RESEARCH/'gpu-bench/target/release/cascii-gpu-bench.exe')]:
    manifest[name]={'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'bytes':p.stat().st_size}
mp=RESEARCH/'results/artifact-manifest.json';mp.write_text(json.dumps(manifest,indent=2),encoding='utf8')
archive=ROOT.parent/'cascii-gpu-investigation.zip'
with zipfile.ZipFile(archive,'w',zipfile.ZIP_DEFLATED,compresslevel=6) as z:
    for p in files+[mp]:z.write(p,p.relative_to(ROOT))
print(f'Validated Markdown links; packaged {len(files)+1} files to {archive} ({archive.stat().st_size:,} bytes).')
