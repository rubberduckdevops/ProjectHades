#!/usr/bin/env node
import * as cdk from 'aws-cdk-lib';
import { HadesStack } from '../lib/hades-stack';

const app = new cdk.App();

new HadesStack(app, 'HadesStack', {
  env: {
    account: process.env.CDK_DEFAULT_ACCOUNT,
    region: process.env.CDK_DEFAULT_REGION ?? 'us-west-2',
  },
});
