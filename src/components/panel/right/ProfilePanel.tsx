import { useState, useCallback } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { Upload, RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { toast } from 'react-toastify';
import Slider from '../../ui/Slider';
import ProfileBrowser from '../../ui/ProfileBrowser';
import type { Adjustments, ProfileSelection } from '../../../utils/adjustments';
import { useProfiles, ImportFailure } from '../../../hooks/useProfiles';
import Text from '../../ui/Text';
import { TextVariants, TextWeights } from '../../../types/typography';

interface ProfilePanelProps {
  adjustments: Adjustments;
  setAdjustments: (value: Partial<Adjustments> | ((prev: Adjustments) => Adjustments)) => void;
  imagePath: string | null;
  onDragStateChange?: (isDragging: boolean) => void;
}

export default function ProfilePanel({
  adjustments,
  setAdjustments,
  imagePath,
  onDragStateChange,
}: ProfilePanelProps) {
  const { t } = useTranslation();
  const { browserModel, importProfiles } = useProfiles(imagePath);
  const [isImporting, setIsImporting] = useState(false);

  const profile: ProfileSelection = adjustments.profile || { base: null, look: null, amount: 100 };
  const hasLook = !!profile.look;
  const hasAnyProfile = !!(profile.base || profile.look);

  const handleSelectBase = useCallback(
    (id: string) => {
      setAdjustments((prev: Adjustments) => ({
        ...prev,
        profile: {
          base: id,
          look: null,
          amount: 100,
        },
      }));
    },
    [setAdjustments],
  );

  const handleSelectLook = useCallback(
    (uuid: string, baseId: string) => {
      setAdjustments((prev: Adjustments) => ({
        ...prev,
        profile: {
          base: baseId,
          look: uuid,
          amount: prev.profile?.amount ?? 100,
        },
      }));
    },
    [setAdjustments],
  );

  const handleClear = useCallback(() => {
    setAdjustments((prev: Adjustments) => ({
      ...prev,
      profile: {
        base: null,
        look: null,
        amount: 100,
      },
    }));
  }, [setAdjustments]);

  const handleAmountChange = useCallback(
    (value: number) => {
      setAdjustments((prev: Adjustments) => ({
        ...prev,
        profile: {
          base: prev.profile?.base ?? null,
          look: prev.profile?.look ?? null,
          amount: value,
        },
      }));
    },
    [setAdjustments],
  );

  const handleImport = useCallback(async () => {
    try {
      const selected = await open({
        multiple: true,
        filters: [
          {
            name: t('profiles.importFilterName'),
            extensions: ['dcp', 'xmp'],
          },
        ],
      });

      if (!selected || (Array.isArray(selected) && selected.length === 0)) return;

      const paths = Array.isArray(selected) ? selected : [selected];
      setIsImporting(true);

      try {
        const result = await importProfiles(paths);
        if (result.imported > 0) {
          toast.success(
            t('profiles.importSuccess', { count: result.imported }),
          );
        }
        if (result.failed.length > 0) {
          const failures = result.failed
            .map((f: ImportFailure) => `${f.path}: ${f.reason}`)
            .join('\n');
          toast.error(`${t('profiles.importFailed')}\n${failures}`);
        }
        if (result.imported === 0 && result.failed.length === 0) {
          toast.info(t('profiles.importNone'));
        }
      } finally {
        setIsImporting(false);
      }
    } catch (err) {
      console.error('Import dialog error:', err);
      setIsImporting(false);
    }
  }, [importProfiles, t]);

  return (
    <div className="mb-2">
      <div className="flex justify-between items-center mb-1">
        <Text variant={TextVariants.title} weight={TextWeights.normal}>
          {t('profiles.title')}
        </Text>
        <div className="flex items-center gap-1">
          <button
            className="p-1.5 rounded-full hover:bg-surface transition-colors"
            onClick={handleImport}
            disabled={isImporting}
            data-tooltip={t('profiles.importTooltip')}
          >
            <Upload size={16} className={isImporting ? 'opacity-50' : ''} />
          </button>
          {hasAnyProfile && (
            <button
              className="p-1.5 rounded-full hover:bg-surface transition-colors"
              onClick={handleClear}
              data-tooltip={t('profiles.resetTooltip')}
            >
              <RotateCcw size={16} />
            </button>
          )}
        </div>
      </div>

      <ProfileBrowser
        model={browserModel}
        activeBase={profile.base}
        activeLook={profile.look}
        onSelectBase={handleSelectBase}
        onSelectLook={handleSelectLook}
        onClear={handleClear}
        className="mb-2"
      />

      {hasLook && (
        <Slider
          label={t('profiles.amount')}
          max={200}
          min={0}
          onChange={(e: React.ChangeEvent<HTMLInputElement> | { target: { value: number | string } }) => {
            const val = parseFloat(String(e.target.value));
            handleAmountChange(val);
          }}
          step={1}
          value={profile.amount}
          defaultValue={100}
          trackClassName="bg-surface"
          onDragStateChange={onDragStateChange}
        />
      )}
    </div>
  );
}
