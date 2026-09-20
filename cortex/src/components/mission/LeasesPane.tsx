import { useState } from 'react';
import { GitBranch, Loader2 } from 'lucide-react';
import { getImpactSet, type ImpactResponse } from '../../lib/cortexApi';

/**
 * Leases — pane four of mission control (SURFACE.md), backed by CONTEXT.md C4.
 *
 * The enterprise question this answers: if I change these files, what else am
 * I claiming, and why? Lease *enforcement* has always existed; what was
 * missing is knowing what to claim. A lease over the paths someone thought to
 * list lets two runs collide at merge time, after both have spent money.
 *
 * Deliberately a query tool rather than a live board of held leases. Showing
 * currently-held leases needs an endpoint that does not exist yet; showing an
 * empty board would imply nothing is held, which is a different claim from
 * "we have not asked".
 */
export default function LeasesPane() {
  const [input, setInput] = useState('');
  const [depth, setDepth] = useState(1);
  const [result, setResult] = useState<ImpactResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    const files = input
      .split(/[,\n]/)
      .map((part) => part.trim())
      .filter(Boolean);
    if (files.length === 0) return;

    setLoading(true);
    setError(null);
    try {
      setResult(await getImpactSet(files, depth));
    } catch (err) {
      setError(err instanceof Error ? err.message : 'could not compute the impact set');
      setResult(null);
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="mx-auto max-w-3xl p-6">
      <h1 className="flex items-center gap-2 text-lg font-semibold text-white">
        <GitBranch className="h-4 w-4" />
        Leases
      </h1>
      <p className="mt-2 text-sm leading-relaxed text-[var(--muted)]">
        What a change actually touches. Enter the files a task would edit and
        Cortex computes the bounded dependency closure a lease should claim —
        with the route that put each file there.
      </p>

      <div className="mt-4 flex flex-col gap-2">
        <textarea
          value={input}
          onChange={(event) => setInput(event.target.value)}
          placeholder={'crates/api/src/state.rs\ncortex/src/App.tsx'}
          rows={3}
          className="w-full resize-y rounded-lg border border-white/10 bg-black/20 px-3 py-2 font-mono text-xs text-white placeholder:text-[var(--muted)] focus:border-[var(--accent)] focus:outline-none"
        />
        <div className="flex items-center gap-3">
          <label className="flex items-center gap-2 text-xs text-[var(--muted)]">
            Depth
            <select
              value={depth}
              onChange={(event) => setDepth(Number(event.target.value))}
              className="rounded border border-white/10 bg-black/20 px-2 py-1 text-xs text-white"
            >
              {[1, 2, 3].map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            onClick={() => void run()}
            disabled={loading || input.trim().length === 0}
            className="inline-flex items-center gap-1.5 rounded-lg bg-[var(--accent)] px-3 py-1.5 text-xs font-medium text-white transition hover:opacity-90 disabled:opacity-40"
          >
            {loading && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
            Compute impact
          </button>
        </div>
        <p className="text-[11px] text-[var(--muted)]">
          One hop is the files that break if you get it wrong. Deeper claims more
          and blocks more — a lease that claims too little causes a merge
          conflict, one that claims too much stops other people working.
        </p>
      </div>

      {error && (
        <p className="mt-4 rounded-lg border border-red-500/20 bg-red-500/10 px-3 py-2 text-xs text-red-300">
          {error}
        </p>
      )}

      {result && (
        <div className="mt-5">
          <p className="text-xs text-[var(--muted)]">
            {result.files.length} file{result.files.length === 1 ? '' : 's'} claimed
            at depth {result.depth}, from {result.files_indexed} indexed.
            {result.truncated && (
              <span className="text-amber-300">
                {' '}
                Stopped at the depth limit — the real blast radius is wider.
              </span>
            )}
          </p>
          <ul className="mt-2 rounded-lg border border-white/8 bg-white/[0.02]">
            {result.files.map((file) => (
              <li
                key={file.file}
                className="border-b border-white/5 px-3 py-2 last:border-b-0"
              >
                <p className="font-mono text-xs text-white">{file.file}</p>
                <p className="mt-0.5 text-[11px] text-[var(--muted)]">
                  {file.distance === 0 ? 'edited directly' : `via ${file.via}`}
                </p>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
