#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * Trying a branch by building it.
 *
 * A pull request used to be published as a release of its own and installed
 * like an update, which meant a second rolling tag, a workflow to keep it
 * moving and a third kind of build that could break on its own. This does the
 * same job with none of that: fetch the branch, build the bundle the same way
 * the nightly is built, and hand the result to the branch's own installer.
 * What comes out is a nightly like any other, at the nightly's paths, and the
 * update button is the way back to master.
 *
 * The machine needs Docker; Git comes from the installed bundle. Nothing here
 * is done for a release build: an installed nightly is the only thing this can
 * replace.
 */

/*
 * What was pasted in, as something git can fetch.
 *
 * Understood, all naming the same thing when they name the same thing:
 *
 *   https://github.com/owner/xd/pull/128        a pull request, any repository
 *   https://github.com/owner/xd/pull/128/files  and the tabs GitHub links to
 *   #128, 128                                   a pull request on this one
 *   https://github.com/owner/xd/tree/some/work  a branch, any repository
 *   some/work                                   a branch on this one
 *
 * A pull request is fetched as refs/pull/N/head, which the base repository
 * carries even when the branch itself lives in a fork -- so a fork needs no
 * special case, and neither does a branch that has been force-pushed.
 *
 * @url, @ref and @label are set on success and untouched otherwise. Anything
 * that is not one of these -- a sentence, a shell fragment, a ref name git
 * would reject -- is FALSE rather than something to guess at, because what
 * comes out of here is run.
 */
gboolean  xd_branch_build_parse (const char  *text,
                                 char       **url,
                                 char       **ref,
                                 char       **label);

/* Where the source is kept between builds, so a rebuild is a fetch. */
char     *xd_branch_build_checkout_dir (void);

/*
 * The shell that fetches @ref from @url into @checkout, builds it and installs
 * it over this copy.
 *
 * One script rather than a sequence of spawns: every step is the previous one
 * having worked, `set -e` says that once, and what runs is a thing that can be
 * read in a terminal when it fails. The install is the checkout's own
 * scripts/install.sh, so where things go stays decided in one place -- and it
 * is the branch's copy of it, which is the copy that matches what was built.
 */
char     *xd_branch_build_command (const char *url,
                                   const char *ref,
                                   const char *checkout);

G_END_DECLS
