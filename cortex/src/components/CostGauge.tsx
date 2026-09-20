import type { CostsState } from '../types';

function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return String(n);
}

export default function CostGauge({ data }: { data: CostsState | null }) {
  if (!data) {
    return (
      <div className="bg-neutral-900 rounded-lg p-6 border border-neutral-800">
        <h2 className="text-lg font-semibold text-white mb-4">Cost Gauge</h2>
        <p className="text-neutral-500 text-sm">Waiting for state data...</p>
      </div>
    );
  }

  const providerEntries = Object.entries(data.byProvider);
  const totalCost = data.session.estimatedCostUsd;

  return (
    <div className="bg-neutral-900 rounded-lg p-6 border border-neutral-800">
      <h2 className="text-lg font-semibold text-white mb-4">Cost Gauge</h2>

      <div className="text-center mb-4">
        <span className="text-3xl font-bold text-white">${totalCost.toFixed(2)}</span>
        <p className="text-neutral-400 text-xs mt-1">session total</p>
      </div>

      <div className="grid grid-cols-3 gap-4 mb-4 text-center">
        <div>
          <p className="text-sm font-mono text-neutral-300">{formatTokens(data.session.inputTokens)}</p>
          <p className="text-xs text-neutral-500">input</p>
        </div>
        <div>
          <p className="text-sm font-mono text-neutral-300">{formatTokens(data.session.outputTokens)}</p>
          <p className="text-xs text-neutral-500">output</p>
        </div>
        <div>
          <p className="text-sm font-mono text-neutral-300">{formatTokens(data.session.totalTokens)}</p>
          <p className="text-xs text-neutral-500">total</p>
        </div>
      </div>

      {providerEntries.length > 0 && (
        <div className="space-y-2">
          {providerEntries.map(([provider, info]) => {
            const pct = totalCost > 0 ? (info.estimatedCostUsd / totalCost) * 100 : 0;
            return (
              <div key={provider}>
                <div className="flex justify-between text-xs mb-1">
                  <span className="text-neutral-300 capitalize">{provider}</span>
                  <span className="text-neutral-400">${info.estimatedCostUsd.toFixed(2)} ({formatTokens(info.tokens)})</span>
                </div>
                <div className="w-full bg-neutral-700 rounded-full h-1.5">
                  <div
                    className="h-1.5 rounded-full bg-purple-500 transition-all"
                    style={{ width: `${pct}%` }}
                  />
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
