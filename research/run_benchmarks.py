"""Run from the repository root. Standard-library-only, sequential measured workloads."""
import argparse, hashlib, json, pathlib, random, statistics, subprocess, time
ROOT = pathlib.Path(__file__).resolve().parents[1]
RES = ROOT / 'research/results'
FIX = ROOT / 'research/fixtures'
BIN = ROOT / 'research/gpu-bench/target/release/cascii-gpu-bench.exe'
BASE = ROOT / 'target/release/cascii.exe'

def run(cmd, log=None):
    t = time.perf_counter()
    p = subprocess.run([str(x) for x in cmd], cwd=ROOT, capture_output=True)
    ms = (time.perf_counter()-t)*1000
    if log:
        log.write_text(p.stdout.decode(errors='replace')+'\n'+p.stderr.decode(errors='replace'), encoding='utf8')
    if p.returncode:
        raise RuntimeError(f'{cmd}\n{p.stderr.decode(errors="replace")}')
    return ms, p.stdout.decode(errors='replace')

def compare_dirs(a, b):
    aa, bb = sorted(a.rglob('*.cframe')), sorted(b.rglob('*.cframe'))
    assert len(aa)==len(bb)==120, (a,b,len(aa),len(bb))
    identical = sum(hashlib.sha256(x.read_bytes()).digest()==hashlib.sha256(y.read_bytes()).digest() for x,y in zip(aa,bb))
    mismatch = count = total = maxerr = 0
    for i in [0,29,59,89,119]:
        x,y=aa[i].read_bytes(),bb[i].read_bytes()
        assert x[:8]==y[:8] and len(x)==len(y)
        for k in range(8,len(x),4):
            count += 1
            mismatch += x[k]!=y[k]
            for j in [1,2,3]:
                d=abs(x[k+j]-y[k+j]);total+=d;maxerr=max(maxerr,d)
    return dict(byte_identical_frames=identical, frames=120, sampled_indices=[0,29,59,89,119], sampled_cells=count,
                char_mismatch_pct=mismatch/count*100, rgb_mae=total/(count*3), rgb_max_abs=maxerr)

def videos():
    results=[]
    run_id=time.strftime('%Y%m%d-%H%M%S')
    modes=['baseline','stream_cpu_small','stream_gpu_small','stream_cpu_full','stream_gpu_full','cuda_scale_cpu','cuda_scale_gpu']
    for label,w,h in [('1080p',1920,1080),('4k',3840,2160)]:
        source=FIX/f'motion_{label}.mp4'
        outputs={}
        for rep in range(4):
            order=modes.copy();random.Random(20261110+rep).shuffle(order)
            for mode in order:
                out=RES/'video-output'/run_id/f'{label}_{mode}_{rep}'
                out.mkdir(parents=True,exist_ok=False)
                if mode=='baseline':
                    cmd=[BASE,source,'--out',out,'--default','--columns','400','--fps','30','--font-ratio','0.7','--luminance','20','--color-only']
                else:
                    backend='gpu' if mode.endswith('gpu') or '_gpu_' in mode else 'cpu'
                    decode='cuda' if mode.startswith('cuda') else 'cpu'
                    small='0' if mode.endswith('full') else '1'
                    cmd=[BIN,'video',backend,source,w,h,400,decode,small,out]
                ms,stdout=run(cmd,RES/f'video_{label}_{mode}_{rep}.log')
                detail={} if mode=='baseline' else json.loads(stdout)
                assert len(list(out.rglob('*.cframe')))==120
                row=dict(clip=label,mode=mode,rep=rep,warmup=rep==0,wall_ms=ms,detail=detail,command=[str(x) for x in cmd])
                results.append(row);outputs[mode]=out
                (RES/'videos.json').write_text(json.dumps(results,indent=2))
                print(label,rep,mode,round(ms,2),flush=True)
        quality={mode:compare_dirs(outputs['baseline'],outputs[mode]) for mode in modes if mode!='baseline'}
        quality['full_cpu_vs_full_gpu']=compare_dirs(outputs['stream_cpu_full'],outputs['stream_gpu_full'])
        quality['cuda_cpu_vs_cuda_gpu']=compare_dirs(outputs['cuda_scale_cpu'],outputs['cuda_scale_gpu'])
        (RES/f'video_quality_{label}.json').write_text(json.dumps(quality,indent=2))
    summary=[]
    for label in ['1080p','4k']:
        for mode in modes:
            vals=[r['wall_ms'] for r in results if r['clip']==label and r['mode']==mode and not r['warmup']]
            summary.append(dict(clip=label,mode=mode,median_ms=statistics.median(vals),min_ms=min(vals),max_ms=max(vals),fps=120000/statistics.median(vals)))
    (RES/'video_summary.json').write_text(json.dumps(summary,indent=2))

def fitters():
    results=[]
    run_id=time.strftime('%Y%m%d-%H%M%S')
    for columns in [80,160,400]:
        for mode,flags in [('off',[]),('legacy',['--fit-cell-backgrounds']),('optimized',['--fit-cell-backgrounds-optimized'])]:
            for rep in range(4):
                out=RES/'fit-output'/run_id/f'{columns}_{mode}_{rep}'
                out.mkdir(parents=True,exist_ok=False)
                cmd=[BASE,ROOT/'resources/test_01.png','--out',out,'--default','--columns',columns,'--font-ratio','0.7','--luminance','20','--color-only',*flags]
                ms,_=run(cmd,RES/f'fit_{columns}_{mode}_{rep}.log')
                results.append(dict(columns=columns,mode=mode,rep=rep,warmup=rep==0,wall_ms=ms,command=[str(x) for x in cmd]))
                (RES/'fitters.json').write_text(json.dumps(results,indent=2))
                print('fit',columns,mode,rep,round(ms,2),flush=True)

def images():
    results=[]
    run_id=time.strftime('%Y%m%d-%H%M%S')
    for name in ['test_01.png','source.png']:
        for rep in range(4):
            order=['baseline','cpu','gpu'];random.Random(rep+71).shuffle(order)
            for mode in order:
                out=RES/'image-output'/run_id/f'{name}_{mode}_{rep}'
                out.mkdir(parents=True,exist_ok=False)
                source=ROOT/'resources'/name
                cmd=([BASE,source,'--out',out,'--default','--columns',400,'--font-ratio','0.7','--luminance',20,'--color-only']
                     if mode=='baseline' else [BIN,'image',mode,source,out/'frame.cframe'])
                ms,stdout=run(cmd,RES/f'image_{name}_{mode}_{rep}.log')
                assert len(list(out.rglob('*.cframe')))==1
                results.append(dict(image=name,mode=mode,rep=rep,warmup=rep==0,wall_ms=ms,detail={} if mode=='baseline' else json.loads(stdout),command=[str(x) for x in cmd]))
                print('image',name,mode,rep,round(ms,2),flush=True)
    (RES/'images.json').write_text(json.dumps(results,indent=2))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('stage',choices=['frames','videos','fitters','images']);args=parser.parse_args()
    RES.mkdir(parents=True,exist_ok=True)
    if args.stage=='frames':
        _,out=run([BIN,'frames'],RES/'frames-run.log');json.loads(out);(RES/'frames.json').write_text(out)
    elif args.stage=='videos':videos()
    elif args.stage=='fitters':fitters()
    else:images()
