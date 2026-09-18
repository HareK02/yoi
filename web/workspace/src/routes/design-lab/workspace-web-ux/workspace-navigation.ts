export const designLabBasePath = '/design-lab/workspace-web-ux';

export type WorkspaceNavigationItem = {
  label: string;
  href: string;
  children?: WorkspaceNavigationItem[];
};

export const workspaceNavigation: WorkspaceNavigationItem[] = [
  { label: 'Tickets', href: `${designLabBasePath}?resource=tickets` },
  { label: 'Objectives', href: `${designLabBasePath}?resource=objectives` },
  { label: 'Merge Requests', href: `${designLabBasePath}?resource=merge-requests` },
  {
    label: 'Memory',
    href: `${designLabBasePath}?resource=memory`,
    children: [
      { label: 'Document', href: `${designLabBasePath}?resource=memory-document` },
      { label: 'Staging', href: `${designLabBasePath}?resource=memory-staging` },
    ],
  },
  { label: 'Workers', href: `${designLabBasePath}?resource=workers` },
];

export const workspaceWorkers = [
  {
    key: 'wrk-language-review',
    label: 'Language review',
    state: 'Running',
    repository: 'yoi',
  },
  {
    key: 'wrk-accessibility-check',
    label: 'Accessibility check with a deliberately long display name',
    state: 'Idle',
    repository: 'workspace-web',
  },
] as const;
