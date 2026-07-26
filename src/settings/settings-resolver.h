#pragma once

#include <glib.h>

#include "tree/xd-node.h"

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
} XdEffectiveSettings;

void                 xd_effective_settings_free (XdEffectiveSettings *self);

/* @default_backend is the last resort when no folder in the chain names one. */
XdEffectiveSettings *xd_settings_resolve        (XdNode     *folder,
                                                 const char *default_backend);

/*
 * Where a chat is, for the assistant answering it.
 *
 * A folder in this tree is an organisational thing -- often an empty directory
 * whose whole content is the dotfile that gives it an id -- so an agent
 * started in one and told nothing else reasonably concludes it has been
 * pointed at the wrong place, and goes looking. Saying which folder it is in
 * and what that directory is costs one line and answers the question.
 *
 * NULL when there is no folder to describe.
 */
char                *xd_settings_describe_place (XdNode     *folder,
                                                 const char *workdir);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (XdEffectiveSettings, xd_effective_settings_free)

G_END_DECLS
