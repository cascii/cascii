"""Verify output files separately from timed runs and capture fixture hashes."""
import hashlib,json,pathlib,struct
ROOT=pathlib.Path(__file__).resolve().parents[1];RES=ROOT/'research/results'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
files=list((ROOT/'resources').glob('*.png'))+list((ROOT/'research/fixtures').glob('*.mp4'))
(RES/'fixtures.json').write_text(json.dumps([dict(path=str(p.relative_to(ROOT)),bytes=p.stat().st_size,sha256=sha(p)) for p in files],indent=2))
fits=json.loads((RES/'fitters.json').read_text()); checks=[]
for cols in [80,160,400]:
    paths=[]
    for mode in ['legacy','optimized']:
        row=next(r for r in fits if r['columns']==cols and r['mode']==mode and r['rep']==3)
        out=pathlib.Path(row['command'][row['command'].index('--out')+1]);paths.append(next(out.rglob('*.cframe')))
    same=paths[0].read_bytes()==paths[1].read_bytes();assert same
    checks.append(dict(columns=cols,legacy_vs_optimized_byte_identical=same,sha256=sha(paths[0])))
images=json.loads((RES/'images.json').read_text());ic=[]
for name in ['test_01.png','source.png']:
    paths={}
    for mode in ['baseline','cpu','gpu']:
        row=next(r for r in images if r['image']==name and r['mode']==mode and r['rep']==3)
        cmd=row['command'];out=pathlib.Path(cmd[cmd.index('--out')+1]) if mode=='baseline' else pathlib.Path(cmd[-1]).parent
        paths[mode]=next(out.rglob('*.cframe'))
    a=paths['baseline'].read_bytes();b=paths['cpu'].read_bytes();c=paths['gpu'].read_bytes()
    assert a==b;assert len(a)==len(c) and a[:8]==c[:8]
    chars=sum(a[i]!=c[i] for i in range(8,len(a),4));delta=[abs(a[i]-c[i]) for i in range(8,len(a)) if (i-8)%4!=0]
    ic.append(dict(image=name,cpu_matches_upstream=True,gpu_char_mismatch_pct=chars/((len(a)-8)//4)*100,gpu_rgb_max_abs=max(delta),gpu_rgb_mae=sum(delta)/len(delta)))
(RES/'artifact-verification.json').write_text(json.dumps(dict(fitters=checks,images=ic),indent=2))
print('Verified fitter parity at three sizes and CPU image parity; hashed all fixtures.')
