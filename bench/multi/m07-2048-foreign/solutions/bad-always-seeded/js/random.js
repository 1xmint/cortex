// The seed from ?seed=N in the address, or null when there is none.
function seedFromAddress() {
  var match = /[?&]seed=(\d+)/.exec(window.location.search);
  return match ? Number(match[1]) : null;
}

// A random number source: seeded (mulberry32) when there is a seed,
// Math.random when there is not.
function makeRandom(seed) {
  if (seed === null) seed = 1;
  var state = seed >>> 0;
  return function () {
    state = (state + 0x6D2B79F5) >>> 0;
    var t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
