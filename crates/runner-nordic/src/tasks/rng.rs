// Copyright 2022 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use wasefire_board_api as board;

impl board::rng::Api for &mut crate::tasks::Board {
    fn fill_bytes(&mut self, buffer: &mut [u8]) -> Result<(), board::Error> {
        critical_section::with(|cs| self.0.borrow_ref_mut(cs).rng.random(buffer));
        Ok(())
    }
}
