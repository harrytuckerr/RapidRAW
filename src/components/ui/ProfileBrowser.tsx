import { useState, useEffect, useRef, useMemo, useCallback } from 'react';
import { AnimatePresence, motion } from 'framer-motion';
import { Check, ChevronDown } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import clsx from 'clsx';
import Input from './Input';
import Text from './Text';
import { TextVariants, TextColors, TextWeights } from '../../types/typography';

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

// ---------------------------------------------------------------------------
// Search-filtered items
// ---------------------------------------------------------------------------

interface DcpItem {
  kind: 'dcp';
  id: string;
  profileName: string;
  groupIndex: number;
}

interface LookItem {
  kind: 'look';
  uuid: string;
  name: string;
  group: string;
  pairing: PairingState;
  requiredBaseProfile: string;
  groupIndex: number;
}

type BrowserItem = DcpItem | LookItem;

function isSatisfied(pairing: PairingState): boolean {
  return pairing.state === 'satisfied';
}

function pairingMessage(entry: LookBrowserEntry): string | null {
  const st = entry.pairing;
  if (st.state === 'satisfied') return null;
  if (st.state === 'missing_base') {
    const installed = st.installed_for_camera.length > 0
      ? ` Installed profiles for ${st.camera}: ${st.installed_for_camera.join(', ')}.`
      : ` No profiles are installed for ${st.camera}.`;
    return `'${entry.name}' requires the base profile '${st.required_name}' for ${st.camera}, which is not installed.${installed}`;
  }
  if (st.state === 'no_profiles_for_camera') {
    return `No camera profiles are installed for ${st.camera}. Install a base DCP for this camera to use Looks.`;
  }
  return null;
}

interface ProfileBrowserProps {
  model: ProfileBrowserModel | null;
  activeBase: string | null;
  activeLook: string | null;
  onSelectBase: (id: string) => void;
  onSelectLook: (uuid: string, baseId: string) => void;
  onClear: () => void;
  className?: string;
}

export default function ProfileBrowser({
  model,
  activeBase,
  activeLook,
  onSelectBase,
  onSelectLook,
  onClear,
  className = '',
}: ProfileBrowserProps) {
  const { t } = useTranslation();
  const [isOpen, setIsOpen] = useState(false);
  const [searchTerm, setSearchTerm] = useState('');
  const containerRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const [highlightIndex, setHighlightIndex] = useState(-1);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(event.target as Node)) {
        setIsOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, []);

  useEffect(() => {
    if (!isOpen) {
      setSearchTerm('');
      setHighlightIndex(-1);
    }
  }, [isOpen]);

  useEffect(() => {
    if (isOpen && searchInputRef.current) {
      searchInputRef.current.focus();
    }
  }, [isOpen]);

  // ---- Build flat filtered list ----
  const filteredItems = useMemo((): BrowserItem[] => {
    if (!model) return [];
    const term = searchTerm.toLowerCase().trim();
    const items: BrowserItem[] = [];

    // Camera profiles group
    for (const group of model.cameraProfiles) {
      for (const entry of group.entries) {
        if (!term || entry.profileName.toLowerCase().includes(term)) {
          items.push({
            kind: 'dcp',
            id: entry.id,
            profileName: entry.profileName,
            groupIndex: 0,
          });
        }
      }
    }

    // Looks grouped by their `group` field
    const lookGroups = new Map<string, LookBrowserEntry[]>();
    for (const look of model.looks) {
      const g = look.group || 'Looks';
      if (!lookGroups.has(g)) lookGroups.set(g, []);
      lookGroups.get(g)!.push(look);
    }

    let groupIdx = 1;
    for (const [groupName, looks] of lookGroups) {
      for (const look of looks) {
        if (!term || look.name.toLowerCase().includes(term) || groupName.toLowerCase().includes(term)) {
          items.push({
            kind: 'look',
            uuid: look.uuid,
            name: look.name,
            group: groupName,
            pairing: look.pairing,
            requiredBaseProfile: look.requiredBaseProfile,
            groupIndex: groupIdx,
          });
        }
      }
      groupIdx++;
    }

    return items;
  }, [model, searchTerm]);

  // ---- Active label ----
  const activeLabel = useMemo(() => {
    if (!model) return t('profiles.placeholder');
    if (activeLook) {
      const look = model.looks.find((l) => l.uuid === activeLook);
      if (look) return look.name;
    }
    if (activeBase) {
      for (const group of model.cameraProfiles) {
        const entry = group.entries.find((e) => e.id === activeBase);
        if (entry) return entry.profileName;
      }
    }
    return t('profiles.placeholder');
  }, [model, activeBase, activeLook, t]);

  // ---- Keyboard ----
  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (!isOpen) {
        if (e.key === 'Enter' || e.key === ' ' || e.key === 'ArrowDown') {
          e.preventDefault();
          setIsOpen(true);
        }
        return;
      }

      if (e.key === 'Escape') {
        e.stopPropagation();
        setIsOpen(false);
        return;
      }

      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setHighlightIndex((prev) => Math.min(prev + 1, filteredItems.length - 1));
        return;
      }

      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setHighlightIndex((prev) => Math.max(prev - 1, 0));
        return;
      }

      if (e.key === 'Enter' && highlightIndex >= 0 && highlightIndex < filteredItems.length) {
        e.preventDefault();
        const item = filteredItems[highlightIndex];
        handleSelectItem(item);
        return;
      }
    },
    [isOpen, filteredItems, highlightIndex],
  );

  const handleSelectItem = useCallback(
    (item: BrowserItem) => {
      if (item.kind === 'dcp') {
        onSelectBase(item.id);
      } else {
        if (!isSatisfied(item.pairing)) return;
        const baseId = item.pairing.state === 'satisfied' ? item.pairing.base : '';
        onSelectLook(item.uuid, baseId);
      }
      setIsOpen(false);
    },
    [onSelectBase, onSelectLook],
  );

  // ---- Group labelling ----
  const needsGroupLabel = useCallback(
    (item: BrowserItem, idx: number): boolean => {
      if (idx === 0) {
        if (item.kind === 'dcp' && model?.cameraProfiles && model.cameraProfiles.length > 0) return true;
        if (item.kind === 'look') return true;
        return false;
      }
      const prev = filteredItems[idx - 1];
      if (item.kind === 'dcp' && prev.kind !== 'dcp') return true;
      if (item.kind === 'look' && (prev.kind !== 'look' || prev.group !== item.group)) return true;
      return false;
    },
    [filteredItems, model],
  );

  const groupLabelText = useCallback(
    (item: BrowserItem): string => {
      if (item.kind === 'dcp') {
        return model?.camera
          ? t('profiles.cameraProfilesFor', { camera: model.camera })
          : t('profiles.cameraProfiles');
      }
      return item.group;
    },
    [model, t],
  );

  const noResults = model && filteredItems.length === 0;

  return (
    <div className={`relative ${className}`} ref={containerRef} onKeyDown={handleKeyDown}>
      <button
        aria-expanded={isOpen}
        aria-haspopup="listbox"
        className={clsx(
          'w-full border border-border-color rounded-md px-3 py-2.5 flex justify-between items-center text-left',
          'min-h-[44px]',
          'focus:ring-accent focus:border-accent focus:outline-hidden focus:ring-2',
          'bg-surface hover:bg-card-active transition-colors',
        )}
        onClick={() => setIsOpen(!isOpen)}
        type="button"
      >
        <Text as="span" variant={TextVariants.label} color={TextColors.primary}>
          {activeLabel}
        </Text>
        <ChevronDown
          className={`text-text-secondary transition-transform duration-200 ${isOpen ? 'rotate-180' : ''}`}
          size={20}
        />
      </button>

      <AnimatePresence>
        {isOpen && (
          <motion.div
            animate={{ opacity: 1, scale: 1 }}
            className="absolute left-0 right-0 mt-1 z-30 origin-top"
            exit={{ opacity: 0, scale: 0.95 }}
            initial={{ opacity: 0, scale: 0.95 }}
            transition={{ duration: 0.1, ease: 'easeOut' }}
          >
            <div
              className="bg-surface/95 backdrop-blur-md rounded-lg shadow-xl max-h-80 overflow-hidden flex flex-col"
              role="listbox"
              ref={listRef}
            >
              {/* Search */}
              <div className="p-2 pb-0 shrink-0">
                <Input
                  ref={searchInputRef}
                  value={searchTerm}
                  onChange={(e: React.ChangeEvent<HTMLInputElement>) => {
                    setSearchTerm(e.target.value);
                    setHighlightIndex(-1);
                  }}
                  placeholder={t('profiles.searchPlaceholder')}
                  className="w-full"
                />
              </div>

              {/* No results */}
              {noResults && (
                <div className="p-4 text-center">
                  <Text variant={TextVariants.label} color={TextColors.secondary}>
                    {t('profiles.noResults')}
                  </Text>
                </div>
              )}

              {/* Items */}
              <div className="overflow-y-auto p-2">
                {filteredItems.map((item, idx) => {
                  const isDisabled = item.kind === 'look' && !isSatisfied(item.pairing);
                  const isActive =
                    (item.kind === 'dcp' && item.id === activeBase && !activeLook) ||
                    (item.kind === 'look' && item.uuid === activeLook);
                  const showGroupLabel = needsGroupLabel(item, idx);
                  const msg =
                    item.kind === 'look' && !isSatisfied(item.pairing)
                      ? pairingMessage(
                          model?.looks.find((l) => l.uuid === item.uuid) || ({
                            name: item.name,
                            requiredBaseProfile: item.requiredBaseProfile,
                            pairing: item.pairing,
                          } as LookBrowserEntry),
                        )
                      : null;

                  return (
                    <div key={item.kind === 'dcp' ? `dcp-${item.id}` : `look-${item.uuid}`}>
                      {showGroupLabel && (
                        <div className="px-2 pt-2 pb-1">
                          <Text variant={TextVariants.label} color={TextColors.secondary} weight={TextWeights.semibold}>
                            {groupLabelText(item)}
                          </Text>
                        </div>
                      )}
                      <button
                        className={clsx(
                          'w-full text-left px-3 py-2.5 rounded-md flex items-center justify-between transition-colors duration-150 min-h-[44px]',
                          {
                            'hover:bg-bg-primary cursor-pointer': !isDisabled,
                            'opacity-40 cursor-not-allowed': isDisabled,
                            'bg-bg-primary': isActive && !isDisabled,
                            'bg-bg-primary/50': highlightIndex === idx,
                          },
                        )}
                        disabled={isDisabled}
                        onClick={() => handleSelectItem(item)}
                        onMouseEnter={() => setHighlightIndex(idx)}
                        role="option"
                        aria-selected={isActive}
                        aria-disabled={isDisabled}
                        aria-label={isDisabled && msg ? msg : undefined}
                        data-tooltip={isDisabled && msg ? msg : undefined}
                      >
                        <Text
                          color={TextColors.primary}
                          weight={isActive ? TextWeights.semibold : TextWeights.normal}
                        >
                          {item.kind === 'dcp' ? item.profileName : item.name}
                        </Text>
                        {isActive && <Check size={16} className="text-accent shrink-0" />}
                      </button>
                    </div>
                  );
                })}

                {/* Clear selection */}
                {(activeBase || activeLook) && (
                  <div className="border-t border-border-color mt-1 pt-1">
                    <button
                      className="w-full text-left px-3 py-2.5 rounded-md hover:bg-bg-primary transition-colors duration-150 min-h-[44px]"
                      onClick={() => {
                        onClear();
                        setIsOpen(false);
                      }}
                      role="option"
                    >
                      <Text variant={TextVariants.label} color={TextColors.secondary}>
                        {t('profiles.clearSelection')}
                      </Text>
                    </button>
                  </div>
                )}
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
