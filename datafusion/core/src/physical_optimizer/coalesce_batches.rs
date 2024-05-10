// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! CoalesceBatches optimizer that groups batches together rows
//! in bigger batches to avoid overhead with small batches

use std::sync::Arc;

use crate::{
    config::ConfigOptions,
    error::Result,
    physical_optimizer::PhysicalOptimizerRule,
    physical_plan::{
        coalesce_batches::CoalesceBatchesExec, filter::FilterExec, joins::HashJoinExec,
        repartition::RepartitionExec, Partitioning,
    },
};

use datafusion_common::tree_node::{Transformed, TransformedResult, TreeNode, TreeNodeRecursion, TreeNodeRewriter, TreeNodeVisitor};
use datafusion_physical_plan::ExecutionPlan;

use super::utils::is_recursive_query;

/// Optimizer rule that introduces CoalesceBatchesExec to avoid overhead with small batches that
/// are produced by highly selective filters
#[derive(Default)]
pub struct CoalesceBatches {}

impl CoalesceBatches {
    #[allow(missing_docs)]
    pub fn new() -> Self {
        Self::default()
    }
}
impl PhysicalOptimizerRule for CoalesceBatches {
    fn optimize(
        &self,
        plan: Arc<dyn crate::physical_plan::ExecutionPlan>,
        config: &ConfigOptions,
    ) -> Result<Arc<dyn crate::physical_plan::ExecutionPlan>> {
        if !config.execution.coalesce_batches {
            return Ok(plan);
        }

        let mut rewriter = CoalesceVisitor {
            target_batch_size: config.execution.batch_size,
        };

        plan.rewrite(& mut rewriter).data()
    }

    fn name(&self) -> &str {
        "coalesce_batches"
    }

    fn schema_check(&self) -> bool {
        true
    }
}


struct CoalesceVisitor {
    target_batch_size: usize,
}

impl TreeNodeRewriter for CoalesceVisitor {
    type Node = Arc<dyn ExecutionPlan>;

    fn f_down(&mut self, node: Self::Node) -> Result<Transformed<Self::Node>> {
        let is_recursive = is_recursive_query(&node);
        if is_recursive {
            Ok(Transformed::new(node, false, TreeNodeRecursion::Jump))
        }
        else {
        Ok(Transformed::no(node))
        }
    }

    fn f_up(&mut self, node: Self::Node) -> Result<Transformed<Self::Node>> {
        let is_recursive = is_recursive_query(&node);
        if is_recursive {
            Ok(Transformed::no(node))
        }
        else {
            let plan = node;
            let plan_any = plan.as_any();
            // The goal here is to detect operators that could produce small batches and only
            // wrap those ones with a CoalesceBatchesExec operator. An alternate approach here
            // would be to build the coalescing logic directly into the operators
            // See https://github.com/apache/arrow-datafusion/issues/139
            let wrap_in_coalesce = plan_any.downcast_ref::<FilterExec>().is_some()
                || plan_any.downcast_ref::<HashJoinExec>().is_some()
                // Don't need to add CoalesceBatchesExec after a round robin RepartitionExec
                || plan_any
                    .downcast_ref::<RepartitionExec>()
                    .map(|repart_exec| {
                        !matches!(
                            repart_exec.partitioning().clone(),
                            Partitioning::RoundRobinBatch(_)
                        )
                    })
                    .unwrap_or(false);
            if wrap_in_coalesce {
                Ok(Transformed::yes(Arc::new(CoalesceBatchesExec::new(
                    plan,
                    self.target_batch_size,
                ))))
            } else {
                Ok(Transformed::no(plan))
            }

        }
        
        
    }



}