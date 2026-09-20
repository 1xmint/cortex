import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';

interface ResizablePanelsProps {
  storageKey: string;
  left: ReactNode;
  right: ReactNode;
  minLeft?: number;
  minRight?: number;
  defaultLeftPercent?: number;
  className?: string;
}

function readRatio(storageKey: string, fallback: number) {
  try {
    const raw = window.localStorage.getItem(storageKey);
    const parsed = raw ? Number.parseFloat(raw) : Number.NaN;
    return Number.isFinite(parsed) ? Math.min(0.78, Math.max(0.32, parsed)) : fallback;
  } catch {
    return fallback;
  }
}

export default function ResizablePanels({
  storageKey,
  left,
  right,
  minLeft = 420,
  minRight = 360,
  defaultLeftPercent = 56,
  className = '',
}: ResizablePanelsProps) {
  const fallbackRatio = defaultLeftPercent / 100;
  const [ratio, setRatio] = useState(() => readRatio(storageKey, fallbackRatio));
  const [dragging, setDragging] = useState(false);
  const containerRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    try {
      window.localStorage.setItem(storageKey, String(ratio));
    } catch {
      // Ignore panel preference persistence failures.
    }
  }, [ratio, storageKey]);

  const updateFromClientX = useCallback((clientX: number) => {
    const rect = containerRef.current?.getBoundingClientRect();
    if (!rect) return;
    const rawLeft = clientX - rect.left;
    const maxLeft = rect.width - minRight;
    const clampedLeft = Math.min(maxLeft, Math.max(minLeft, rawLeft));
    setRatio(clampedLeft / rect.width);
  }, [minLeft, minRight]);

  useEffect(() => {
    if (!dragging) return;
    const onPointerMove = (event: PointerEvent) => updateFromClientX(event.clientX);
    const onPointerUp = () => setDragging(false);
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp);
    document.body.classList.add('cortex-resizing');
    return () => {
      window.removeEventListener('pointermove', onPointerMove);
      window.removeEventListener('pointerup', onPointerUp);
      document.body.classList.remove('cortex-resizing');
    };
  }, [dragging, updateFromClientX]);

  const columns = useMemo(() => `${Math.round(ratio * 1000) / 10}% 10px minmax(0, 1fr)`, [ratio]);

  return (
    <div
      ref={containerRef}
      className={`grid min-h-0 flex-1 ${className}`}
      style={{ gridTemplateColumns: columns }}
    >
      <div className="min-w-0">{left}</div>
      <button
        type="button"
        aria-label="Resize task manager panels"
        title="Drag to resize panels"
        onPointerDown={(event) => {
          event.preventDefault();
          event.currentTarget.setPointerCapture(event.pointerId);
          setDragging(true);
          updateFromClientX(event.clientX);
        }}
        onDoubleClick={() => setRatio(fallbackRatio)}
        className="group hidden cursor-col-resize items-stretch justify-center bg-transparent outline-none lg:flex"
      >
        <span className="my-2 w-px rounded-full bg-white/8 transition group-hover:bg-[var(--accent)]/60 group-focus-visible:bg-[var(--accent)]" />
      </button>
      <div className="min-w-0">{right}</div>
    </div>
  );
}
