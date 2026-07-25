#pragma once

#include <adwaita.h>

#include "storage/storage.h"
#include "tree/fs-tree.h"

G_BEGIN_DECLS

#define HY_TYPE_CHAT_VIEW (hy_chat_view_get_type ())
G_DECLARE_FINAL_TYPE (HyChatView, hy_chat_view, HY, CHAT_VIEW, AdwBin)

/*
 * The transcript of one chat, plus the composer.
 *
 * Passing NULL to hy_chat_view_set_chat() shows the empty state.
 */
HyChatView *hy_chat_view_new      (HyStorage  *storage,
                                   HyFsTree   *tree);

void        hy_chat_view_set_chat (HyChatView *self,
                                   HyNode     *chat);

HyNode     *hy_chat_view_get_chat (HyChatView *self);

G_END_DECLS
