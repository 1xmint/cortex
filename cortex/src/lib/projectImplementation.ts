import type { ConversationFlow } from '../types';

type FlowHandoff = NonNullable<ConversationFlow['handoff']>;

export function createImplementationMessage(handoff: FlowHandoff): string {
  const context = typeof handoff.context === 'string'
    ? handoff.context
    : JSON.stringify(handoff.context, null, 2);

  return [
    `Ready to continue with ${handoff.target}.`,
    '',
    'Implementation context:',
    context,
  ].join('\n');
}
