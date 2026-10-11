/*
 * This file is part of paged (https://paged.media).
 *
 * paged is free software: you may redistribute it and/or modify it under the
 * terms of the GNU Affero General Public License, version 3, as published by
 * the Free Software Foundation, OR under the Paged Media Enterprise License
 * (PMEL), a commercial license available from And The Next GmbH. Full
 * copyright and license information is available in LICENSE.md, distributed
 * with this source code.
 *
 * paged is distributed in the hope that it will be useful, but WITHOUT ANY
 * WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
 * FOR A PARTICULAR PURPOSE. See the licenses for details.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    AGPL-3.0-only OR Paged Media Enterprise License (PMEL)
 */

// A second-click confirm for panel actions that can't be undone. The first
// click arms the button (its label then says "Confirm …"), a second click
// within CONFIRM_MS runs the action, and the arm lapses after that. Never
// window.confirm: the question stays in the panel, on the button itself.

import { useEffect, useRef, useState } from "react";

/** How long an armed button waits for its confirming second click. */
export const CONFIRM_MS = 4000;

export function useConfirm(): {
  /** The key of the armed action, or null. */
  armed: string | null;
  /** Arm `key`, or run `run` when `key` is already armed. */
  confirm: (key: string, run: () => void) => void;
} {
  const [armed, setArmed] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  const confirm = (key: string, run: () => void) => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    if (armed === key) {
      setArmed(null);
      run();
      return;
    }
    setArmed(key);
    timer.current = setTimeout(() => setArmed(null), CONFIRM_MS);
  };
  return { armed, confirm };
}
