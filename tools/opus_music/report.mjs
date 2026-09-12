export function summarize(data, ratings) {
  return data.tracks[0].variants.map(({bitrateKbps: bitrate}) => {
    const samples = data.tracks.map(t => ratings[`${t.id}:gameplay`]?.[bitrate]).filter(Boolean);
    const files = data.tracks.map(t => t.variants.find(v => v.bitrateKbps === bitrate));
    const kbPerMinute = files.reduce((a,v) => a + v.sizeBytes, 0) / files.reduce((a,v) => a + v.durationSeconds, 0) * 60 / 1000;
    const mean = key => samples.length ? samples.reduce((a,r) => a + Number(r[key]),0) / samples.length : null;
    const score = mean('score');
    const accepted = samples.length ? samples.filter(r => r.acceptable === 'yes').length / samples.length : null;
    const percentages = samples.filter(r => r.percent !== '');
    const quality = percentages.length ? percentages.reduce((a,r) => a + Number(r.percent),0) / percentages.length : null;
    return {bitrate, count:samples.length, score, accepted, quality, kbPerMinute,
      qualifies: data.tracks.length >= 3 && samples.length === data.tracks.length && score >= 4 && accepted >= .8 && (quality === null || quality >= 80),
      budgetMinutes: [1000,900,750,500].map(kb => kb / kbPerMinute)};
  });
}
