"""Publication figures from recorded measurements; no benchmark execution."""
import json, pathlib, statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np

ROOT=pathlib.Path(__file__).resolve().parent
RES=ROOT/'results'; OUT=ROOT/'figures';OUT.mkdir(exist_ok=True)
plt.rcParams.update({'font.size':11,'axes.spines.top':False,'axes.spines.right':False,'figure.dpi':150,'savefig.bbox':'tight'})
def save(fig,name):
    fig.savefig(OUT/f'{name}.png');fig.savefig(OUT/f'{name}.svg');plt.close(fig)

frames=json.loads((RES/'frames.json').read_text())
chosen=[r for r in frames['results'] if r['case'].startswith('synthetic') and r['columns']==400]
fig,ax=plt.subplots(figsize=(9,4.5));x=np.arange(len(chosen));bw=.34
for offset,key,color,label in [(-bw/2,'cpu','#52647a','Upstream CPU API'),(bw/2,'gpu','#007f75','Warm GPU, transfers included')]:
    values=[r[key]['median_ms'] for r in chosen]
    bars=ax.bar(x+offset,values,bw,label=label,color=color)
    ax.bar_label(bars,fmt='%.2f',padding=3)
ax.set_xticks(x,['640 × 360','1920 × 1080','3840 × 2160']);ax.set_ylabel('Milliseconds per frame (lower is better)')
ax.set_title('Decoded RGB → 400-column .cframe\n15 samples; GPU initialization and image decoding excluded')
ax.legend();ax.set_ylim(0,max(r['cpu']['median_ms'] for r in chosen)*1.28)
save(fig,'frame-compute')

summary=json.loads((RES/'video_summary.json').read_text())
labels=['Upstream PNG + Rayon','CPU stream, early scale','GPU stream, early scale','CPU stream, full RGB','GPU stream, full RGB','CUDA scale + CPU','CUDA scale + GPU']
fig,axes=plt.subplots(1,2,figsize=(12,5.4),sharey=True)
for ax,clip in zip(axes,['1080p','4k']):
    rows=[r for r in summary if r['clip']==clip];values=np.array([r['median_ms']/1000 for r in rows])
    lo=np.array([r['min_ms']/1000 for r in rows]);hi=np.array([r['max_ms']/1000 for r in rows])
    bars=ax.barh(np.arange(7),values,color=['#52647a','#a5b1bf','#007f75','#a5b1bf','#007f75','#a5b1bf','#007f75'],xerr=[values-lo,hi-values],capsize=3)
    ax.bar_label(bars,fmt='%.3f s',padding=4,fontsize=9)
    ax.set_title(clip.upper());ax.set_xlabel('Seconds, including process/device setup')
    ax.set_xlim(0,max(hi)*1.23);ax.set_yticks(np.arange(7),labels)
axes[0].invert_yaxis()
fig.suptitle('120-frame H.264 clips → 400-column color frames\nMedian and min–max of 3 runs; full RGB and CUDA paths change resampling',fontsize=13)
fig.tight_layout();save(fig,'video-end-to-end')

images=json.loads((RES/'images.json').read_text())
fig,ax=plt.subplots(figsize=(9,4.5));x=np.arange(2);bw=.24
for j,(mode,label,color) in enumerate([('baseline','Upstream CLI','#52647a'),('cpu','CPU prototype','#a5b1bf'),('gpu','GPU prototype, fresh device','#007f75')]):
    vals=[statistics.median(r['wall_ms'] for r in images if r['image']==name and r['mode']==mode and not r['warmup']) for name in ['test_01.png','source.png']]
    bars=ax.bar(x+(j-1)*bw,vals,bw,label=label,color=color);ax.bar_label(bars,fmt='%.0f',padding=3)
ax.set_xticks(x,['test_01.png','source.png']);ax.set_ylabel('Milliseconds, process launch through output file')
ax.set_title('One image per process: GPU startup is part of the job\nMedian of 3 runs after one filesystem warmup');ax.legend()
ax.set_ylim(0,ax.get_ylim()[1]*1.2);save(fig,'cold-images')

# A compact diagnostic: shader arithmetic differences, not a perceptual quality score.
fig,ax=plt.subplots(figsize=(10,4.5))
rows=[r for r in frames['results'] if r['columns']==400]
ax.bar([r['case'].replace('synthetic_','') for r in rows],[r['quality']['char_mismatch_pct'] for r in rows],color='#a15c2f')
ax.set_ylabel('ASCII cells differing from CPU (%)');ax.set_title('Warm frame output fidelity at 400 columns\nMaximum RGB-channel difference: 1/255 on tested fixtures')
ax.tick_params(axis='x',rotation=25);save(fig,'frame-fidelity')
