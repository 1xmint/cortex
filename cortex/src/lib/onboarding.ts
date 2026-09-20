const ONBOARDING_KEY_PREFIX = 'cortex:onboarding-complete:';
const ONBOARDING_STEP_KEY_PREFIX = 'cortex:onboarding-step:';

export function isOnboardingComplete(userId: string): boolean {
  try {
    return window.localStorage.getItem(`${ONBOARDING_KEY_PREFIX}${userId}`) === 'true';
  } catch {
    return false;
  }
}

export function markOnboardingComplete(userId: string): void {
  try {
    window.localStorage.setItem(`${ONBOARDING_KEY_PREFIX}${userId}`, 'true');
    window.localStorage.removeItem(`${ONBOARDING_STEP_KEY_PREFIX}${userId}`);
  } catch {
    // Onboarding should not block the app if local storage is unavailable.
  }
}

export function saveOnboardingStep(userId: string, step: number): void {
  try {
    window.localStorage.setItem(`${ONBOARDING_STEP_KEY_PREFIX}${userId}`, step.toString());
  } catch {
    // Ignore storage failures
  }
}

export function getOnboardingStep(userId: string): number {
  try {
    const saved = window.localStorage.getItem(`${ONBOARDING_STEP_KEY_PREFIX}${userId}`);
    return saved ? parseInt(saved, 10) : 0;
  } catch {
    return 0;
  }
}
