// @vitest-environment jsdom
import '@testing-library/jest-dom/vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../lib/cortexApi', async () => {
  const actual = await vi.importActual<typeof import('../../lib/cortexApi')>('../../lib/cortexApi');
  return {
    ...actual,
    getChatModels: vi.fn(),
  };
});

import { getChatModels, type ChatModelEntry } from '../../lib/cortexApi';
import { saveZenDeviceKey } from '../../lib/zenDeviceKey';
import ModelPicker from './ModelPicker';

afterEach(() => {
  vi.clearAllMocks();
  cleanup();
  window.localStorage.clear();
});

const MODELS: ChatModelEntry[] = [
  { provider: 'claude', model: 'claude-fast', label: 'fast', billing: 'credits', available: true, unavailable_reason: null },
  { provider: 'claude', model: 'claude-balanced', label: 'balanced', billing: 'credits', available: true, unavailable_reason: null },
  { provider: 'zen', model: 'zen:glm', label: 'glm', billing: 'your_zen_key', available: false, unavailable_reason: 'needs_key' },
  { provider: 'zen', model: 'zen:kimi', label: 'kimi', billing: 'your_zen_key', available: true, unavailable_reason: null },
];

describe('ModelPicker', () => {
  it('marks a Zen entry disabled when available is false, and available ones selectable', async () => {
    vi.mocked(getChatModels).mockResolvedValue({ models: MODELS });
    const onSelect = vi.fn();

    render(<ModelPicker selectedModel={undefined} onSelect={onSelect} />);
    fireEvent.click(screen.getByRole('button', { name: /cortex credits/i }));

    await waitFor(() => expect(screen.getByText('glm')).toBeInTheDocument());
    const glmOption = screen.getByRole('option', { name: /glm/i });
    expect(glmOption).toHaveAttribute('aria-disabled', 'true');

    const kimiOption = screen.getByRole('option', { name: /kimi/i });
    expect(kimiOption).not.toHaveAttribute('aria-disabled', 'true');
    fireEvent.click(kimiOption);
    expect(onSelect).toHaveBeenCalledWith('zen:kimi');
  });

  it('routes a disabled Zen entry to Settings instead of selecting it', async () => {
    vi.mocked(getChatModels).mockResolvedValue({ models: MODELS });
    const onSelect = vi.fn();
    const onOpenModelSettings = vi.fn();

    render(<ModelPicker selectedModel={undefined} onSelect={onSelect} onOpenModelSettings={onOpenModelSettings} />);
    fireEvent.click(screen.getByRole('button', { name: /cortex credits/i }));

    await waitFor(() => expect(screen.getByText('glm')).toBeInTheDocument());
    fireEvent.click(screen.getByRole('option', { name: /glm/i }));

    expect(onOpenModelSettings).toHaveBeenCalledTimes(1);
    expect(onSelect).not.toHaveBeenCalled();
  });

  it('shows the 409 zen_key_required message with a Settings link, and never auto-switches or retries', async () => {
    vi.mocked(getChatModels).mockResolvedValue({ models: MODELS });
    const onSelect = vi.fn();
    const onOpenModelSettings = vi.fn();

    render(
      <ModelPicker
        selectedModel="zen:kimi"
        onSelect={onSelect}
        onOpenModelSettings={onOpenModelSettings}
        zenKeyError="kimi runs on your own OpenCode Zen key. Add it in Settings → Model keys."
      />,
    );

    expect(screen.getByRole('alert')).toHaveTextContent(/runs on your own OpenCode Zen key/);
    fireEvent.click(screen.getByRole('button', { name: /open settings/i }));
    expect(onOpenModelSettings).toHaveBeenCalledTimes(1);
    // The picker itself never calls onSelect on its own in response to the error.
    expect(onSelect).not.toHaveBeenCalled();
    expect(getChatModels).toHaveBeenCalledTimes(1);
  });

  it('fetches models without a device header when this browser has no local Zen device key', async () => {
    vi.mocked(getChatModels).mockResolvedValue({ models: MODELS });

    render(<ModelPicker selectedModel={undefined} onSelect={vi.fn()} />);

    await waitFor(() => expect(getChatModels).toHaveBeenCalledTimes(1));
    expect(getChatModels).toHaveBeenCalledWith(undefined);
  });

  it('fetches models with this device\'s id (never a secret) when a local Zen device key exists', async () => {
    vi.mocked(getChatModels).mockResolvedValue({ models: MODELS });
    saveZenDeviceKey('local', { deviceId: 'device-abc', secret: 'super-secret-value' });

    render(<ModelPicker selectedModel={undefined} onSelect={vi.fn()} />);

    await waitFor(() => expect(getChatModels).toHaveBeenCalledTimes(1));
    expect(getChatModels).toHaveBeenCalledWith('device-abc');
  });
});
