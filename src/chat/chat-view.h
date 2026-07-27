#pragma once

#include <adwaita.h>

#include "remote/client.h"
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

/* The top bar, exposed only so the window can size it with the sidebar's bar. */
GtkWidget  *xd_chat_view_get_header (XdChatView *self);

void        xd_chat_view_set_chat (XdChatView *self,
                                   XdNode     *chat);

/*
 * A chat that lives on a daemon, read over @client.
 *
 * The transcript is the same transcript, drawn the same way. What is not there
 * is everything that acts on this machine: the composer, the terminal, the
 * working tree. The daemon takes no messages over the wire yet, and a composer
 * that swallowed what was typed into it would be worse than none.
 */
void        xd_chat_view_show_remote_chat (XdChatView     *self,
                                           XdNode         *chat,
                                           XdRemoteClient *client);

XdNode     *xd_chat_view_get_chat (XdChatView *self);

G_END_DECLS
