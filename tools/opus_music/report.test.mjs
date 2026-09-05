import {test} from 'node:test';
import assert from 'node:assert/strict';
import {summarize} from './report.mjs';
const data={tracks:['a','b','c'].map(id=>({id,variants:[{bitrateKbps:24,sizeBytes:180000,durationSeconds:60}]}))};
test('unrated and partial experiments cannot recommend a bitrate',()=>{
  assert.equal(summarize(data,{})[0].qualifies,false);
  assert.equal(summarize(data,{'a:gameplay':{24:{score:5,acceptable:'yes',percent:''}}})[0].qualifies,false);
});
test('gameplay acceptance, optional fidelity and measured budgets gate recommendation',()=>{
  const ratings=Object.fromEntries(data.tracks.map(t=>[`${t.id}:gameplay`,{24:{score:4,acceptable:'yes',percent:'85'}}]));
  let row=summarize(data,ratings)[0];
  assert.equal(row.qualifies,true); assert.equal(row.budgetMinutes[1],5);
  ratings['a:gameplay'][24].acceptable='maybe';
  assert.equal(summarize(data,ratings)[0].qualifies,false);
  ratings['a:gameplay'][24].acceptable='yes'; ratings['a:gameplay'][24].percent='50';
  assert.equal(summarize(data,ratings)[0].qualifies,false);
});
