#pragma once

#include <adwaita.h>

#include "storage/storage.h"
#include "tree/fs-tree.h"

G_BEGIN_DECLS

#define XD_TYPE_CHAT_VIEW (xd_chat_view_get_type ())
G_DECLARE_FINAL_TYPE (XdChatView, xd_chat_view, XD, CHAT_VIEW, AdwBin)

/*
 * The transcript of one chat, plus the composer.
 *
 * Passing NULL to xd_chat_view_set_chat() shows the empty state.
 */
XdChatView *xd_chat_view_new      (XdStorage  *storage,
                                   XdFsTree   *tree);

void        xd_chat_view_set_chat (XdChatView *self,
                                   XdNode     *chat);

XdNode     *xd_chat_view_get_chat (XdChatView *self);

G_END_DECLS
