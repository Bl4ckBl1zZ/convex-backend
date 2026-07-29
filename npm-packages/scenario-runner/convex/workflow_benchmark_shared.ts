export function mixChecksum(
  parent: number,
  level: number,
  ordinal: number,
  work: number,
) {
  let value = (parent ^ Math.imul(level + 1, 0x9e3779b1) ^ ordinal) >>> 0;
  for (let i = 0; i < work; i += 1) {
    value =
      (Math.imul(value ^ (value >>> 16), 0x85ebca6b) +
        Math.imul(i + 1, 0xc2b2ae35)) >>>
      0;
  }
  return value;
}
