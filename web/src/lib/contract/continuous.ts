// Continuous control paths: api/ws/model.schema.json#/$defs/ContinuousPath.
// Writes to these may be coalesced and rate limited; writes to every other
// control path are discrete and sent each, in order. The contract test
// checks that these patterns are the schema's.

const ID = '[a-z][a-z0-9-]{0,31}';

export const CONTINUOUS_PATH_PATTERNS: readonly string[] = [
  `^/(channels|buses|auxes|mixMinus)/${ID}/fader$`,
  `^/channels/${ID}/(trim|legTrim/[0-7]|hpf/freq|pan/(lr|fr)|sends/${ID}/(level|pan))$`,
  `^/(channels|buses|auxes)/${ID}/(eq/bands/[0-3]/(freq|gain|q)|gate/(threshold|range|attack|release)|comp/(threshold|ratio|attack|release|makeup)|delay)$`,
  `^/vcas/${ID}/offset$`,
  '^/monitor/level$',
];

const CONTINUOUS = CONTINUOUS_PATH_PATTERNS.map((p) => new RegExp(p));

export function isContinuousPath(path: string): boolean {
  return CONTINUOUS.some((re) => re.test(path));
}
