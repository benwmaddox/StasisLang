import {summarize} from './report.mjs';
const $ = id => document.getElementById(id);
let data, state, key, order, generation = 0;
let pendingSwitch = null;
const player = $('player');
const persist = () => localStorage.setItem(key, JSON.stringify(state));
const current = () => data.tracks[Number($('track').value)];
const ratingKey = () => `${current().id}:${$('mode').value}`;
function shuffle(values) {
  for (let i=values.length-1;i>0;i--) {
    const j=Math.floor(Math.random()*(i+1));
    [values[i],values[j]]=[values[j],values[i]];
  }
  return values;
}
async function switchTo(path, button) {
  const token = ++generation;
  const position = pendingSwitch?.position ?? (player.currentTime || 0);
  const playing = pendingSwitch?.playing ?? !player.paused;
  pendingSwitch = {position, playing};
  player.pause();
  player.src = path;
  $('status').textContent = `Selected ${button.textContent}`;
  player.onloadedmetadata = async () => {
    if (token !== generation) return;
    pendingSwitch = null;
    player.currentTime = Math.min(position, Math.max(0,player.duration-.01));
    if (playing) {
      try { await player.play(); } catch (error) { $('status').textContent = error.message; }
    }
  };
  player.load();
}
function render() {
  generation++; pendingSwitch = null; player.onloadedmetadata = null;
  player.pause(); player.removeAttribute('src'); player.load();
  const track=current();
  order=state.orders[track.id];
  const saved=state.ratings[ratingKey()];
  $('versions').replaceChildren(); $('fields').replaceChildren();
  const button = (label,path) => {
    const b=document.createElement('button'); b.textContent=label;
    b.onclick=()=>switchTo(path,b); $('versions').append(b); return b;
  };
  button('Reference',track.reference);
  for (const [index,bitrate] of order.entries()) {
    const label=`Version ${String.fromCharCode(65+index)}${saved ? ` (${bitrate} kbps)` : ''}`;
    button(label,track.variants.find(v=>v.bitrateKbps===bitrate).path);
    const field=document.createElement('fieldset');
    field.innerHTML=`<legend>${label}</legend>
      <label>Quality <select name="score-${bitrate}" required><option value="">Choose</option><option value="1">1 - unacceptable</option><option value="2">2 - poor</option><option value="3">3 - usable</option><option value="4">4 - good</option><option value="5">5 - no problem</option></select></label>
      <label>Would use in a game? <select name="acceptable-${bitrate}" required><option value="">Choose</option><option value="yes">Yes</option><option value="maybe">Maybe</option><option value="no">No</option></select></label>
      <label>Source quality % (optional) <input name="percent-${bitrate}" type="number" min="0" max="100"></label>`;
    $('fields').append(field);
    if(saved) for(const name of ['score','acceptable','percent']) field.querySelector(`[name="${name}-${bitrate}"]`).value=saved[bitrate][name];
  }
  player.volume=$('mode').value==='gameplay' ? 10**(-15/20) : 1;
  $('status').textContent=saved ? 'Saved ratings revealed for this mode.' : 'Bitrates hidden until all versions in this mode are scored.';
  report();
}
function report() {
  const rows=summarize(data,state.ratings);
  const winner=rows.filter(r=>r.qualifies).sort((a,b)=>a.bitrate-b.bitrate)[0];
  $('recommendation').textContent=winner ? `Lowest qualifying bitrate: ${winner.bitrate} kbps mono VBR. Confirm with more styles before adopting. Percent quality is optional; missing percentages do not establish 80% fidelity.` : 'No recommendation yet: need at least three tracks, complete gameplay ratings, average score >=4 and >=80% Yes. Supplied quality percentages must average >=80%.';
  const fmt = v => v===null ? '-' : v.toFixed(1);
  $('report').innerHTML='<table><thead><tr><th>kbps</th><th>Gameplay n / score / Yes %</th><th>KB/min</th><th>Minutes: 1 MB / 900 / 750 / 500 KB</th></tr></thead><tbody>'+rows.map(r=>`<tr><td>${r.bitrate}</td><td>${r.count} / ${fmt(r.score)} / ${fmt(r.accepted===null?null:r.accepted*100)}</td><td>${fmt(r.kbPerMinute)}</td><td>${r.budgetMinutes.map(fmt).join(' / ')}</td></tr>`).join('')+'</tbody></table><p>Decimal KB and MB; sizes include the Opus container. Results are from one local listener.</p>';
}
$('ratings').onsubmit=event=>{
  event.preventDefault();
  const values=new FormData(event.target), result={};
  for(const bitrate of order) result[bitrate]=Object.fromEntries(['score','acceptable','percent'].map(name=>[name,values.get(`${name}-${bitrate}`)]));
  state.ratings[ratingKey()]=result;
  try { persist(); render(); } catch(error) { $('status').textContent=`Storage failed: ${error.message}. Export ratings before closing.`; }
};
$('track').onchange=render; $('mode').onchange=render;
$('restart').onclick=()=>{player.currentTime=0;};
player.onerror=()=>{$('status').textContent='Audio could not load. Check the build and browser Opus support.';};
function download(name,text,type) {
  const url=URL.createObjectURL(new Blob([text],{type}));
  const a=document.createElement('a'); a.href=url; a.download=name; a.click();
  setTimeout(()=>URL.revokeObjectURL(url),1000);
}
$('export').onclick=()=>{
  download('ratings.json',JSON.stringify({experiment:data, ...state, summary:summarize(data,state.ratings)},null,2),'application/json');
  download('summary.html','<!doctype html><meta charset="utf-8"><title>Opus results</title><h1>Low-bitrate Opus results</h1><p>'+$('recommendation').textContent+'</p>'+$('report').innerHTML,'text/html');
};
try {
  const response=await fetch('metadata.json',{cache:'no-store'});
  if(!response.ok) throw new Error('Add masters and run build_test.py first (see README).');
  data=await response.json(); key=`opus-music-v1:${data.experimentId}`;
  state=JSON.parse(localStorage.getItem(key)||'null') || {orders:{},ratings:{}};
  for(const [index,track] of data.tracks.entries()) {
    const option=document.createElement('option'); option.value=index; option.textContent=track.name; $('track').append(option);
    state.orders[track.id] ||= shuffle(track.variants.map(v=>v.bitrateKbps));
  }
  persist(); render();
} catch(error) { $('status').textContent=error.message; }
