import * as fs from 'fs';
import * as path from 'path';
import {
  Annotations,
  CfnOutput,
  Duration,
  Fn,
  RemovalPolicy,
  Stack,
  StackProps,
} from 'aws-cdk-lib';
import * as dynamodb from 'aws-cdk-lib/aws-dynamodb';
import * as cognito from 'aws-cdk-lib/aws-cognito';
import * as lambda from 'aws-cdk-lib/aws-lambda';
import * as apigwv2 from 'aws-cdk-lib/aws-apigatewayv2';
import { HttpLambdaIntegration } from 'aws-cdk-lib/aws-apigatewayv2-integrations';
import * as s3 from 'aws-cdk-lib/aws-s3';
import * as s3deploy from 'aws-cdk-lib/aws-s3-deployment';
import * as cloudfront from 'aws-cdk-lib/aws-cloudfront';
import * as origins from 'aws-cdk-lib/aws-cloudfront-origins';
import { RustFunction } from 'cargo-lambda-cdk';
import { Construct } from 'constructs';

export class HadesStack extends Stack {
  constructor(scope: Construct, id: string, props?: StackProps) {
    super(scope, id, props);

    // ---- DynamoDB -------------------------------------------------------
    const table = new dynamodb.Table(this, 'JournalTable', {
      tableName: 'hades_journal',
      partitionKey: { name: 'pk', type: dynamodb.AttributeType.STRING },
      sortKey: { name: 'sk', type: dynamodb.AttributeType.STRING },
      billingMode: dynamodb.BillingMode.PAY_PER_REQUEST,
      pointInTimeRecoverySpecification: { pointInTimeRecoveryEnabled: true },
      removalPolicy: RemovalPolicy.RETAIN,
    });

    // ---- Cognito --------------------------------------------------------
    const userPool = new cognito.UserPool(this, 'UserPool', {
      userPoolName: 'hades-users',
      signInAliases: { email: true },
      selfSignUpEnabled: false,
      autoVerify: { email: true },
      accountRecovery: cognito.AccountRecovery.EMAIL_ONLY,
      removalPolicy: RemovalPolicy.RETAIN,
    });

    const userPoolClient = userPool.addClient('WebClient', {
      userPoolClientName: 'hades-web',
      generateSecret: false,
      authFlows: { userSrp: true, userPassword: true },
    });

    // ---- Lambda (Rust / Axum) -------------------------------------------
    const fn = new RustFunction(this, 'ApiFunction', {
      manifestPath: path.join(__dirname, '../../backend/Cargo.toml'),
      architecture: lambda.Architecture.ARM_64,
      memorySize: 256,
      timeout: Duration.seconds(10),
      environment: {
        TABLE_NAME: table.tableName,
        COGNITO_USER_POOL_ID: userPool.userPoolId,
        COGNITO_CLIENT_ID: userPoolClient.userPoolClientId,
        // AWS_REGION is set automatically by the Lambda runtime.
      },
    });
    table.grantReadWriteData(fn);

    // ---- HTTP API -------------------------------------------------------
    const httpApi = new apigwv2.HttpApi(this, 'HttpApi', {
      apiName: 'hades-api',
      // $default route -> Lambda. Auth is enforced in-app (Cognito JWT).
      defaultIntegration: new HttpLambdaIntegration('LambdaIntegration', fn),
    });

    // apiEndpoint looks like https://<id>.execute-api.<region>.amazonaws.com
    const apiDomain = Fn.select(2, Fn.split('/', httpApi.apiEndpoint));

    // ---- Frontend hosting ----------------------------------------------
    const siteBucket = new s3.Bucket(this, 'SiteBucket', {
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      enforceSSL: true,
      encryption: s3.BucketEncryption.S3_MANAGED,
      removalPolicy: RemovalPolicy.DESTROY,
      autoDeleteObjects: true,
    });

    // SPA fallback: rewrite extension-less paths to /index.html. Attached to
    // the S3 (default) behavior only so API 404s are not masked.
    const spaRewrite = new cloudfront.Function(this, 'SpaRewriteFunction', {
      runtime: cloudfront.FunctionRuntime.JS_2_0,
      code: cloudfront.FunctionCode.fromInline(`
function handler(event) {
  var request = event.request;
  var uri = request.uri;
  var lastSegment = uri.substring(uri.lastIndexOf('/') + 1);
  if (lastSegment.indexOf('.') === -1) {
    request.uri = '/index.html';
  }
  return request;
}
`),
    });

    const distribution = new cloudfront.Distribution(this, 'Distribution', {
      defaultRootObject: 'index.html',
      defaultBehavior: {
        origin: origins.S3BucketOrigin.withOriginAccessControl(siteBucket),
        viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        functionAssociations: [
          {
            function: spaRewrite,
            eventType: cloudfront.FunctionEventType.VIEWER_REQUEST,
          },
        ],
      },
      additionalBehaviors: {
        '/api/*': {
          origin: new origins.HttpOrigin(apiDomain, {
            protocolPolicy: cloudfront.OriginProtocolPolicy.HTTPS_ONLY,
          }),
          viewerProtocolPolicy: cloudfront.ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
          cachePolicy: cloudfront.CachePolicy.CACHING_DISABLED,
          originRequestPolicy: cloudfront.OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
          allowedMethods: cloudfront.AllowedMethods.ALLOW_ALL,
        },
      },
    });

    // ---- Frontend deployment --------------------------------------------
    const distDir = path.join(__dirname, '../../frontend/dist');
    if (fs.existsSync(distDir)) {
      new s3deploy.BucketDeployment(this, 'DeploySite', {
        destinationBucket: siteBucket,
        sources: [
          // A local-dev public/config.json would be copied into dist; never ship it.
          s3deploy.Source.asset(distDir, { exclude: ['config.json', 'config.example.json'] }),
          s3deploy.Source.jsonData('config.json', {
            apiUrl: '/api',
            region: this.region,
            userPoolId: userPool.userPoolId,
            userPoolClientId: userPoolClient.userPoolClientId,
          }),
        ],
        distribution,
        distributionPaths: ['/*'],
      });
    } else {
      Annotations.of(this).addWarning(
        `Frontend build not found at ${distDir}; skipping site deployment. ` +
          'Run `npm run build` in frontend/ and redeploy.',
      );
    }

    // ---- Outputs --------------------------------------------------------
    new CfnOutput(this, 'CloudFrontUrl', {
      value: `https://${distribution.distributionDomainName}`,
    });
    new CfnOutput(this, 'HttpApiUrl', { value: httpApi.apiEndpoint });
    new CfnOutput(this, 'TableName', { value: table.tableName });
    new CfnOutput(this, 'UserPoolId', { value: userPool.userPoolId });
    new CfnOutput(this, 'UserPoolClientId', { value: userPoolClient.userPoolClientId });
    new CfnOutput(this, 'Region', { value: this.region });
  }
}
