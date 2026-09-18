import { designLabBasePath } from '../workspace-navigation';

export const settingsBasePath = `${designLabBasePath}/settings`;

export const settingsNavigation = [
  { label: 'Runtimes', href: settingsBasePath },
  { label: 'Configuration Sources', href: `${settingsBasePath}?section=configuration-sources` },
  { label: 'Repositories', href: `${settingsBasePath}?section=repositories` },
  { label: 'Repository Access', href: `${settingsBasePath}?section=repository-access` },
  { label: 'Profile Sources', href: `${settingsBasePath}?section=profile-sources` },
  { label: 'Workspace Identity', href: `${settingsBasePath}?section=workspace-identity` },
] as const;
