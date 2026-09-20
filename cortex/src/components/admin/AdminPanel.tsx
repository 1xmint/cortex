import { ShieldCheck, X } from 'lucide-react';
import AdminView from './AdminView';
import PromoCodeManager from './PromoCodeManager';

interface AdminPanelProps {
  onClose: () => void;
}

export default function AdminPanel({ onClose }: AdminPanelProps) {
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/60 backdrop-blur-sm">
      <div className="my-8 w-full max-w-3xl rounded-2xl border border-white/8 bg-[var(--bg)] shadow-2xl">
        <header className="flex items-center justify-between border-b border-white/6 px-5 py-4">
          <div className="flex items-center gap-2">
            <ShieldCheck className="h-5 w-5 text-[var(--accent)]" />
            <h1 className="text-lg font-semibold text-white">Admin Dashboard</h1>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-lg p-1.5 text-[var(--muted)] transition hover:bg-white/8 hover:text-white"
          >
            <X className="h-5 w-5" />
          </button>
        </header>
        <div className="space-y-6 p-5">
          <AdminView />
          <div className="border-t border-white/6 pt-5">
            <PromoCodeManager />
          </div>
        </div>
      </div>
    </div>
  );
}
