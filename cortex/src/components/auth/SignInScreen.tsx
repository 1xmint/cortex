import { SignIn } from '@clerk/clerk-react';

export default function SignInScreen() {
  return (
    <div className="flex min-h-screen items-center justify-center bg-[var(--bg)]">
      <div className="w-full max-w-md px-4">
        <div className="mb-8 text-center">
          <h1 className="text-2xl font-semibold text-white">Cortex</h1>
          <p className="mt-2 text-sm text-[var(--muted)]">
            Multi-provider AI orchestration
          </p>
        </div>
        <SignIn
          appearance={{
            elements: {
              rootBox: 'w-full',
              card: 'bg-[var(--panel)] border border-white/8 shadow-none',
              headerTitle: 'text-white',
              headerSubtitle: 'text-[var(--muted)]',
              formButtonPrimary: 'bg-[var(--accent-soft)] text-[var(--accent)] hover:bg-[var(--accent-soft)]/80',
              formFieldInput: 'bg-[var(--composer)] border-white/10 text-white',
              formFieldLabel: 'text-[var(--muted-strong)]',
              footerActionLink: 'text-[var(--accent)]',
              dividerLine: 'bg-white/10',
              dividerText: 'text-[var(--muted)]',
              socialButtonsBlockButton: 'bg-white/5 border-white/10 text-white hover:bg-white/10',
              socialButtonsBlockButtonText: 'text-white',
            },
          }}
          routing="hash"
        />
      </div>
    </div>
  );
}
