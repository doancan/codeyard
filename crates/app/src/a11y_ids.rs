//! Compile-time checks of the node id layout: fixed nodes are unique and below the blocks, blocks do not overlap, and
//! rows and text runs sit above them.

use super::*;

const _: () = {
    let blocks = [
        STATUS_ITEM,
        UPDATE_BUTTON,
        TAB_BASE,
        PICK_OPTION,
        FIND_CONTROL,
        COMPLETION_OPTION,
        EXTRA_TREE,
        SCM_GROUP,
        SCM_FILE,
        ABOUT_DETAIL,
        super::search_access::SEARCH_ED_ROW,
        super::search_access::SEARCH_ED_TAB,
        EXT_ROW,
        EXT_PAGE_LINE,
        SCM_REPO,
        EXT_VIEW_NODE,
        INSPECTOR_LINE,
        super::problems_access::SEARCH_ROW,
        super::scm_access::ext::SCM_EXT,
        super::problems_access::PAGE_ROW,
        super::problems_access::PROBLEM_ROW,
        super::problems_access::DEBUG_ROW,
        super::problems_access::TEST_ROW,
        super::problems_access::REMOTE_ROW,
        super::problems_access::MENU_ITEM,
        super::problems_access::ROW_MENU,
        super::problems_access::DIALOG_LINE,
        super::term_tab_access::TERM_TAB,
        ROW_BASE,
    ];
    let mut i = 1;
    while i < blocks.len() {
        assert!(blocks[i - 1] + BLOCK <= blocks[i]);
        i += 1;
    }
    // Fixed nodes, whichever file names them, never share an id (and stay below the blocks).
    use super::search_access::{SEARCH_ED, SEARCH_ED_APPLY, SEARCH_ED_LIST, SEARCH_ED_QUERY, SEARCH_ED_STATUS, SEARCH_ED_UNDO};
    let fixed = [
        ROOT,
        EXPLORER,
        EDITOR,
        STATUS,
        UPDATE_DIALOG,
        UPDATE_PROGRESS,
        TABS,
        NOTICE,
        PICKER,
        PICKER_INPUT,
        PICKER_LIST,
        FIND,
        FIND_QUERY,
        FIND_REPLACE,
        FIND_STATUS,
        COMPLETION,
        TERMINAL,
        SCM,
        SCM_MESSAGE,
        SCM_COMMIT,
        SCM_BRANCH,
        SCM_RESULT,
        SCM_EMPTY,
        SERVICES,
        SERVICES_INFO,
        SERVICES_STATUS,
        SERVICES_INSTALL,
        SERVICES_CANCEL,
        ABOUT_TAB,
        ABOUT,
        ABOUT_COPY,
        SEARCH_ED,
        SEARCH_ED_QUERY,
        SEARCH_ED_STATUS,
        SEARCH_ED_LIST,
        SEARCH_ED_APPLY,
        SEARCH_ED_UNDO,
        SCM_REPOS,
        SCM_CHANGES,
        EXT,
        EXT_TABS,
        EXT_TAB_INSTALLED,
        EXT_TAB_MARKET,
        EXT_SEARCH,
        EXT_STATUS,
        EXT_LIST,
        EXT_PAGE,
        EXT_PAGE_INSTALL,
        INSPECTOR,
    ];
    unique(&[
        &fixed,
        &super::problems_access::FIXED,
        &super::problems_access::PAGE_FIXED,
        &super::problems_access::DEBUG_FIXED,
        &super::problems_access::SIDE_FIXED,
        &super::problems_access::DIALOG_FIXED,
        &super::problems_access::SEARCH_FIXED,
    ]);
    // Rows are ROW_BASE | 32-bit hash; editor and terminal runs sit above them.
    assert!(ROW_BASE + 0xffff_ffff < RUN_BASE && RUN_BASE + 0xffff_ffff < TERM_RUN_BASE);
};

/// Every id of every list is below the blocks and appears once across all the lists.
const fn unique(lists: &[&[NodeId]]) {
    let mut a = 0;
    while a < lists.len() {
        let mut i = 0;
        while i < lists[a].len() {
            let id = lists[a][i].0;
            assert!(id < STATUS_ITEM);
            let mut b = a;
            while b < lists.len() {
                let mut j = if b == a { i + 1 } else { 0 };
                while j < lists[b].len() {
                    assert!(id != lists[b][j].0);
                    j += 1;
                }
                b += 1;
            }
            i += 1;
        }
        a += 1;
    }
}
