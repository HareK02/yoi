export function ownsRoutePath(basePath: string, currentPath: string): boolean {
  if (!basePath || !currentPath) return false;
  return currentPath === basePath || currentPath.startsWith(`${basePath}/`);
}
