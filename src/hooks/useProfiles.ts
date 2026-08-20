import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';

export interface LookBrowserEntry {
  uuid: string;
  name: string;
  group: string;
  requiredBaseProfile: string;
  filePath: string;
  pairing: PairingState;
}

export type PairingState =
  | { state: 'satisfied'; base: string }
  | { state: 'missing_base'; required_name: string; camera: string; installed_for_camera: string[] }
  | { state: 'no_profiles_for_camera'; camera: string };

export interface ProfileEntryView {
  id: string;
  filePath: string;
  uniqueCameraModel: string;
  profileName: string;
  source: string;
}

export interface CameraProfileGroup {
  profileName: string;
  entries: ProfileEntryView[];
}

export interface ProfileBrowserModel {
  camera: string | null;
  cameraProfiles: CameraProfileGroup[];
  looks: LookBrowserEntry[];
}

export interface ImportFailure {
  path: string;
  reason: string;
}

export interface ImportResult {
  imported: number;
  failed: ImportFailure[];
}

export function useProfiles(imagePath: string | null) {
  const [browserModel, setBrowserModel] = useState<ProfileBrowserModel | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const lastPathRef = useRef<string | null>(null);

  const fetchProfiles = useCallback(async (path: string) => {
    if (!path) {
      setBrowserModel(null);
      setError(null);
      return;
    }

    setIsLoading(true);
    setError(null);
    try {
      const model: ProfileBrowserModel = await invoke('list_profiles_for_image', { path });
      setBrowserModel(model);
      lastPathRef.current = path;
    } catch (err) {
      console.error('Failed to list profiles:', err);
      setError(String(err));
      setBrowserModel(null);
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    if (imagePath && imagePath !== lastPathRef.current) {
      fetchProfiles(imagePath);
    } else if (!imagePath) {
      setBrowserModel(null);
      setError(null);
      lastPathRef.current = null;
    }
  }, [imagePath, fetchProfiles]);

  const importProfiles = useCallback(async (paths: string[]): Promise<ImportResult> => {
    try {
      const result: ImportResult = await invoke('import_profiles', { sourcePaths: paths });
      // Re-fetch after import
      if (lastPathRef.current) {
        await fetchProfiles(lastPathRef.current);
      }
      return result;
    } catch (err) {
      console.error('Failed to import profiles:', err);
      throw err;
    }
  }, [fetchProfiles]);

  const removeProfile = useCallback(async (id: string): Promise<void> => {
    try {
      await invoke('remove_profile', { id });
      if (lastPathRef.current) {
        await fetchProfiles(lastPathRef.current);
      }
    } catch (err) {
      console.error('Failed to remove profile:', err);
      throw err;
    }
  }, [fetchProfiles]);

  const rescanProfiles = useCallback(async (): Promise<void> => {
    try {
      await invoke('rescan_profiles');
      if (lastPathRef.current) {
        await fetchProfiles(lastPathRef.current);
      }
    } catch (err) {
      console.error('Failed to rescan profiles:', err);
      throw err;
    }
  }, [fetchProfiles]);

  const resolvePairingMessage = useCallback((entry: LookBrowserEntry): string | null => {
    const state = entry.pairing;
    if (state.state === 'satisfied') return null;
    if (state.state === 'missing_base') {
      const installedList = state.installed_for_camera.length > 0
        ? ` Installed profiles for ${state.camera}: ${state.installed_for_camera.join(', ')}.`
        : ` No profiles are installed for ${state.camera}.`;
      return `'${entry.name}' requires the base profile '${state.required_name}' for ${state.camera}, which is not installed.${installedList}`;
    }
    if (state.state === 'no_profiles_for_camera') {
      return `No camera profiles are installed for ${state.camera}. Install a base DCP for this camera to use Looks.`;
    }
    return null;
  }, []);

  return {
    browserModel,
    isLoading,
    error,
    fetchProfiles,
    importProfiles,
    removeProfile,
    rescanProfiles,
    resolvePairingMessage,
  };
}
