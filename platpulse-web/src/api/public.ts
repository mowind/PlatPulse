import { publicNetwork, publicNetworks, publicNodeDetail, type PublicNetwork, type PublicNode } from './generated'

export async function fetchNetworks(): Promise<PublicNetwork[]> {
  const { data, error } = await publicNetworks()
  if (error || !data) throw new Error((error as { error?: { message?: string } } | undefined)?.error?.message ?? 'Unable to load published Nodes')
  return data
}

export async function fetchNetwork(networkKey: string): Promise<PublicNetwork> {
  const { data, error } = await publicNetwork({ path: { network_key: networkKey } })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to load Network')
  return data
}

export async function fetchNode(nodeId: string): Promise<PublicNode> {
  const { data, error } = await publicNodeDetail({ path: { node_id: nodeId } })
  if (error || !data) throw new Error(error?.error?.message ?? 'Unable to load Node')
  return data
}
