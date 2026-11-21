/** Describe how the current installation can launch its headless entry point. */
export interface LaunchCommand {
  executable: string;
  script: string;
  electron: boolean;
}

export function launchArgs(command: LaunchCommand, directory: string, mode: 'boot' | 'background'): string[] {
  return [command.script, '--data-dir', directory, '__serve', mode];
}

export function launchEnvironment(command: LaunchCommand): NodeJS.ProcessEnv {
  const environment: NodeJS.ProcessEnv = { ...process.env };
  delete environment.NODE_OPTIONS;
  delete environment.NODE_PATH;
  delete environment.ELECTRON_RUN_AS_NODE;
  if (command.electron) environment.ELECTRON_RUN_AS_NODE = '1';
  return environment;
}
