import { setVisibility, type VisibilityRequest, type VisibilityResponse } from './generated'

export async function updateNodeVisibility(
  nodeId: string,
  visibility: VisibilityRequest['visibility'],
  csrfToken: string,
): Promise<VisibilityResponse> {
  const { data, error } = await setVisibility({
    path: { node_id: nodeId },
    body: { visibility },
    headers: { 'X-CSRF-Token': csrfToken },
  })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to update Node visibility')
  return data
}
