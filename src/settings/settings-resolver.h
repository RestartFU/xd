#pragma once

#include <glib.h>

#include "tree/hy-node.h"

G_BEGIN_DECLS

/*
 * What a folder's settings actually amount to once the folder chain has had
 * its say.
 *
 * Scalars take the nearest value going up from the folder, so a child
 * overrides its parent. Instructions are the exception: they accumulate from
 * the root down, because a workspace-wide instruction and a folder-specific
 * one are both meant to apply.
 */
typedef struct
{
  char *backend;
  char *model;
  char *workdir;
  char *repo;
  char *instructions;

  /* Where each scalar came from, for "inherited from …" in the interface.
   * NULL when the folder set it itself or nothing set it. */
  char *backend_from;
  char *model_from;
  char *workdir_from;
  char *repo_from;
} HyEffectiveSettings;

void                 hy_effective_settings_free (HyEffectiveSettings *self);

/* @default_backend is the last resort when no folder in the chain names one. */
HyEffectiveSettings *hy_settings_resolve        (HyNode     *folder,
                                                 const char *default_backend);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (HyEffectiveSettings, hy_effective_settings_free)

G_END_DECLS
